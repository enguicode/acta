#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `DefaultPipeline` de punta a punta con el store SQLite real y fakes de
//! ASR, diarización y LLM.

use acta_audio::processing::DefaultPipeline;
use acta_audio::processing::voiceprints::rename_speaker;
use acta_audio::supervisor::files::append_jsonl;
use acta_audio::writer::{SegmentWriter, SegmentWriterConfig};
use acta_core::detection::{Participant, Platform, SpeakerSignal};
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::minutes::*;
use acta_core::processing::{ProcessingPipeline, ProcessingStep, ProgressSink};
use acta_core::recording::*;
use acta_core::screen::{Capture, CaptureReason, ScreenEvent};
use acta_core::settings::Settings;
use acta_core::time::{MediaMs, UnixMs};
use acta_core::transcription::*;
use acta_core::{PortError, PortResult};
use acta_store::SqliteStore;
use acta_store::embeddings::HashEmbedder;
use async_trait::async_trait;
use std::path::Path;
use std::sync::{Arc, Mutex};

const T0: UnixMs = UnixMs(1_700_000_000_000);

struct FakeAsr;
#[async_trait]
impl Transcriber for FakeAsr {
    async fn transcribe(&self, r: TranscribeRequest, _p: Option<ProgressFn>) -> PortResult<Vec<AsrSegment>> {
        assert!(r.wav_path.exists());
        let s = |a, b, t: &str| AsrSegment { start_ms: a, end_ms: b, text: t.into(), confidence: Some(0.9), words: vec![] };
        Ok(match r.track {
            Track::Mic => vec![s(0, 2_000, "Hola, empezamos con el presupuesto")],
            Track::System => vec![s(2_000, 4_000, "Yo me encargo del informe"), s(4_000, 6_000, "Perfecto, gracias")],
        })
    }
}

struct FakeDiar;
#[async_trait]
impl Diarizer for FakeDiar {
    async fn diarize(&self, _wav: &Path, _o: DiarizeOptions, _p: Option<ProgressFn>) -> PortResult<DiarizationResult> {
        Ok(DiarizationResult {
            turns: vec![
                DiarizationTurn { label: "S0".into(), start_ms: 2_000, end_ms: 4_000 },
                DiarizationTurn { label: "S1".into(), start_ms: 4_000, end_ms: 6_000 },
            ],
            speakers: vec![
                DiarizedSpeaker { label: "S0".into(), embedding: vec![1.0, 0.0, 0.0], total_ms: 2_000 },
                DiarizedSpeaker { label: "S1".into(), embedding: vec![0.0, 1.0, 0.0], total_ms: 2_000 },
            ],
            embedding_model: "fluidaudio-wespeaker-community1-256".into(),
        })
    }
}

#[derive(Default)]
struct FakeMinutes {
    fail: Mutex<bool>,
    inputs: Mutex<Vec<MinutesInput>>,
}
#[async_trait]
impl MinutesGenerator for FakeMinutes {
    async fn generate(&self, input: MinutesInput) -> PortResult<MinutesOutput> {
        if *self.fail.lock().unwrap() {
            return Err(PortError::Unavailable("claude terminó con error".into()));
        }
        self.inputs.lock().unwrap().push(input.clone());
        Ok(MinutesOutput {
            minutes: Minutes {
                meeting_id: input.meeting_id,
                summary_md: "Se habló del presupuesto.".into(),
                decisions: vec![],
                action_items: vec![ActionItem {
                    id: ActionItemId::new(),
                    meeting_id: input.meeting_id,
                    text: "Preparar el informe".into(),
                    owner_name: Some("Luis".into()),
                    owner_speaker_id: None,
                    is_mine: false,
                    due: None,
                    done: false,
                    at: Some(MediaMs(2_500)),
                    capture_ids: vec![],
                }],
                provider: "fake".into(),
                model: None,
                generated_at: T0,
            },
            escaleta: Escaleta {
                temas: vec![Tema {
                    titulo: "Presupuesto".into(),
                    resumen: "r".into(),
                    inicio_ms: 0,
                    fin_ms: 6_000,
                    subtemas: vec![],
                    capturas: vec![],
                }],
            },
            key_moments: vec![KeyMoment { at: MediaMs(3_000), reason: "encargo".into(), topic: None }],
            suggested_title: Some("Presupuesto Q1".into()),
        })
    }
}

struct Sink(Mutex<Vec<(ProcessingStep, f32)>>);
impl ProgressSink for Sink {
    fn progress(&self, _m: MeetingId, step: ProcessingStep, f: f32) {
        self.0.lock().unwrap().push((step, f));
    }
}

struct World {
    _dir: tempfile::TempDir,
    layout: DataLayout,
    store: Arc<SqliteStore>,
    minutes: Arc<FakeMinutes>,
    pipeline: DefaultPipeline,
    id: MeetingId,
}

async fn world() -> World {
    world_with_levels(500, 500).await
}

/// Como `world()` pero con amplitud constante distinta en cada pista (para
/// ejercitar las envolventes de energía con audio concatenado de verdad).
async fn world_with_levels(mic_amp: i16, system_amp: i16) -> World {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path());
    let store = Arc::new(
        SqliteStore::open(&layout.db_path())
            .unwrap()
            .with_layout(layout.clone())
            .with_embedder(Arc::new(HashEmbedder)),
    );
    let id = MeetingId::from_u128(5);
    seed(&layout, &store, id, mic_amp, system_amp).await;
    let minutes = Arc::new(FakeMinutes::default());
    let pipeline = DefaultPipeline {
        layout: layout.clone(),
        settings: Settings::default(),
        store: store.clone(),
        voiceprints: store.clone(),
        search: store.clone(),
        transcriber: Arc::new(FakeAsr),
        diarizer: Arc::new(FakeDiar),
        minutes: minutes.clone(),
        speaker_namer: None,
        cleaner: None,
    };
    World { _dir: dir, layout, store, minutes, pipeline, id }
}

async fn seed(layout: &DataLayout, store: &SqliteStore, id: MeetingId, mic_amp: i16, system_amp: i16) {
    store
        .create_meeting(NewMeeting {
            id,
            title: "Reunión de Teams".into(),
            platform: Platform::Teams,
            app_bundle_id: Some("com.microsoft.teams2".into()),
            started_at: T0,
            language: "es".into(),
        })
        .await
        .unwrap();
    // 6 s de audio en las dos pistas.
    let cfg = SegmentWriterConfig { layout: layout.clone(), meeting_id: id, format: AudioFormat::WORKING, segment_seconds: 30, flush_every_ms: 1_000 };
    let mut w = SegmentWriter::create(cfg, T0).unwrap();
    for n in 0..60u64 {
        for track in Track::ALL {
            let amp = if track == Track::Mic { mic_amp } else { system_amp };
            w.push(&AudioFrame { track, seq: n, pts_us: n * 100_000, sample_rate: 16_000, samples: vec![amp; 1600] }).unwrap();
        }
    }
    w.finish(StopReason::User, T0.plus_ms(6_000)).unwrap();
    // Señales en vivo: Teams marca a Luis hablando de 2 a 4 s; Edu es "yo".
    let sp = layout.live_speakers_path(id);
    append_jsonl(&sp, &SpeakerSignal::Roster { participants: vec![Participant { name: "Edu Navajas".into(), is_self: true }, Participant { name: "Luis".into(), is_self: false }], at: T0 }).unwrap();
    append_jsonl(&sp, &SpeakerSignal::Active { names: vec!["Luis".into()], at: T0.plus_ms(2_000) }).unwrap();
    append_jsonl(&sp, &SpeakerSignal::Active { names: vec![], at: T0.plus_ms(4_000) }).unwrap();
    let sc = layout.live_screen_path(id);
    append_jsonl(&sc, &ScreenEvent::ShareStarted { presenter: Some("Luis".into()), at: T0.plus_ms(1_000) }).unwrap();
    append_jsonl(&sc, &ScreenEvent::ShareEnded { at: T0.plus_ms(5_000) }).unwrap();
    // Una candidata cerca del momento clave (3 s) y otra lejos.
    for (n, at) in [(1u128, 3_200u64), (2, 5_900)] {
        let rel = format!("captures/candidates/{}.jpg", T0.0 + at as i64);
        std::fs::create_dir_all(layout.capture_candidates_dir(id)).unwrap();
        std::fs::write(layout.meeting_dir(id).join(&rel), b"jpg").unwrap();
        store
            .add_capture(Capture {
                id: CaptureId::from_u128(id.0.as_u128() * 10 + n),
                meeting_id: id,
                at: MediaMs(at),
                at_wall: T0.plus_ms(at as i64),
                path: rel,
                reason: CaptureReason::Candidate,
                phash: "0".repeat(16),
                width: 10,
                height: 10,
                keyword: None,
                topic_path: None,
                action_item_ids: vec![],
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn full_run_builds_transcript_minutes_index_and_captures() {
    let w = world().await;
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();

    for t in Track::ALL {
        assert!(w.layout.track_wav(w.id, t).exists());
    }
    assert!(w.layout.mix_wav(w.id).exists());
    let steps: Vec<_> = sink.0.lock().unwrap().iter().filter(|(_, f)| *f == 1.0).map(|(s, _)| *s).collect();
    assert_eq!(steps, ProcessingStep::ORDER.to_vec());

    let d = w.store.get_meeting_detail(w.id).await.unwrap().unwrap();
    assert_eq!(d.meeting.duration.0, 6_000);
    assert_eq!(d.meeting.title, "Presupuesto Q1", "título sugerido (no editado)");
    let names: Vec<_> = d.transcript.utterances.iter().map(|u| d.transcript.speakers.iter().find(|s| s.id == u.speaker_id).unwrap().shown_name().to_owned()).collect();
    assert_eq!(names, vec!["Yo", "Luis", "Speaker 2"]);
    let luis = d.transcript.speakers.iter().find(|s| s.display_name.as_deref() == Some("Luis")).unwrap();
    assert_eq!(luis.name_source, NameSource::AppUi);
    assert_eq!(d.minutes.unwrap().action_items.len(), 1);
    assert!(d.timeline.iter().any(|e| e.kind == TimelineKind::ScreenShare && e.at.0 == 1_000));

    // Capturas: la cercana al momento clave se promueve; la otra se poda.
    assert_eq!(d.captures.len(), 1);
    let c = &d.captures[0];
    assert_eq!((c.reason, c.path.as_str(), c.topic_path.as_deref()), (CaptureReason::LlmPick, "captures/000003200-llm_pick.jpg", Some("1")));
    assert!(w.layout.meeting_dir(w.id).join(&c.path).exists());
    assert!(!w.layout.capture_candidates_dir(w.id).join(format!("{}.jpg", T0.0 + 5_900)).exists());

    let hits = w.store.search(SearchQuery { text: "informe".into(), scope: SearchScope::All, limit: 5 }).await.unwrap();
    assert!(hits.iter().any(|h| h.meeting_id == w.id));
    // El LLM recibe a Edu como "yo".
    let input = w.minutes.inputs.lock().unwrap()[0].clone();
    assert!(input.speakers.iter().any(|s| s.is_me));
}

/// Bleed de punta a punta: audio real concatenado, envolventes leídas de los
/// WAV de la reunión y fusión. El micro lleva el remoto a -30 dB y el ASR lo
/// escribe distinto (sin gemelo textual): no puede salir ningún "Yo".
#[tokio::test]
async fn quiet_mic_with_loud_system_is_bleed_not_me() {
    let w = world_with_levels(160, 5_000).await;
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();
    let d = w.store.get_meeting_detail(w.id).await.unwrap().unwrap();
    assert!(d.transcript.speakers.iter().all(|s| !s.is_me), "el bleed salió como 'Yo'");
    assert_eq!(d.transcript.utterances.len(), 2, "solo las del sistema");
}

/// Lo contrario: el micro domina (hablo yo) y el sistema casi calla.
#[tokio::test]
async fn loud_mic_with_quiet_system_stays_me() {
    let w = world_with_levels(5_000, 160).await;
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();
    let d = w.store.get_meeting_detail(w.id).await.unwrap().unwrap();
    let me = d.transcript.speakers.iter().find(|s| s.is_me).expect("Yo");
    assert!(d.transcript.utterances.iter().any(|u| u.speaker_id == me.id));
}

#[tokio::test]
async fn failed_summary_still_indexes_and_reports_the_step() {
    let w = world().await;
    *w.minutes.fail.lock().unwrap() = true;
    let sink = Sink(Mutex::new(vec![]));
    let e = w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap_err();
    assert!(e.to_string().contains("Resumen"), "{e}");
    let hits = w.store.search(SearchQuery { text: "informe".into(), scope: SearchScope::All, limit: 5 }).await.unwrap();
    assert!(!hits.is_empty(), "se indexa aunque falle el LLM");
    assert_eq!(w.store.list_captures(w.id).await.unwrap().len(), 2, "sin acta no se poda");
    // Reintento solo del resumen y la poda.
    *w.minutes.fail.lock().unwrap() = false;
    w.pipeline.run(w.id, &[ProcessingStep::Summarizing, ProcessingStep::PruningCaptures], &sink).await.unwrap();
    assert!(w.store.get_minutes(w.id).await.unwrap().is_some());
}

#[tokio::test]
async fn confirmed_name_becomes_a_voiceprint_that_names_the_next_meeting() {
    let w = world().await;
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    let s2 = t.speakers.iter().find(|s| s.label == "Speaker 2").unwrap().clone();
    let vp = rename_speaker(&w.layout, w.store.as_ref(), w.store.as_ref(), w.id, s2.id, "Marta".into(), true, T0)
        .await
        .unwrap()
        .expect("huella");
    assert_eq!(w.store.list_voiceprints().await.unwrap()[0].display_name, "Marta");

    // Reprocesar los nombres: Marta sigue confirmada y con la huella.
    w.pipeline.run(w.id, &[ProcessingStep::NamingSpeakers], &sink).await.unwrap();
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    let m = t.speakers.iter().find(|s| s.id == s2.id).unwrap();
    assert_eq!((m.display_name.as_deref(), m.confirmed, m.voiceprint_id), (Some("Marta"), true, Some(vp)));

    // Una reunión nueva con la misma voz se sugiere como Marta (sin confirmar).
    let id2 = MeetingId::from_u128(6);
    seed(&w.layout, &w.store, id2, 500, 500).await;
    w.pipeline.run(id2, &ProcessingStep::ORDER, &sink).await.unwrap();
    let t2 = w.store.get_transcript(id2).await.unwrap().unwrap();
    let sug = t2.speakers.iter().find(|s| s.diarization_label.as_deref() == Some("S1")).unwrap();
    assert_eq!((sug.display_name.as_deref(), sug.name_source, sug.confirmed), (Some("Marta"), NameSource::Voiceprint, false));
}

/// Contrato (`ReprocessRequest.steps`): vacío = todos desde `Transcribing`.
/// Antes no se ejecutaba NADA y la reunión seguía tal cual (visto en E2E real).
#[tokio::test]
async fn empty_steps_reprocess_everything_from_transcribing() {
    let w = world().await;
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();
    assert_eq!(w.minutes.inputs.lock().unwrap().len(), 1);
    sink.0.lock().unwrap().clear();
    w.pipeline.run(w.id, &[], &sink).await.unwrap();
    assert_eq!(w.minutes.inputs.lock().unwrap().len(), 2, "el resumen se rehace");
    let steps: Vec<ProcessingStep> = sink.0.lock().unwrap().iter().filter(|(_, f)| *f == 1.0).map(|(s, _)| *s).collect();
    assert!(!steps.contains(&ProcessingStep::Concatenating));
    for s in [ProcessingStep::Transcribing, ProcessingStep::Diarizing, ProcessingStep::NamingSpeakers, ProcessingStep::Summarizing, ProcessingStep::Indexing] {
        assert!(steps.contains(&s), "falta {s:?}: {steps:?}");
    }
}

/// La nota personal del usuario llega al generador de actas como contexto.
#[tokio::test]
async fn the_users_note_reaches_the_minutes_generator() {
    let w = world().await;
    w.pipeline
        .store
        .patch_meeting(w.id, MeetingPatch { user_note: Some(Some("Pedir descuento".into())), ..Default::default() })
        .await
        .unwrap();
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &Sink(Mutex::new(vec![]))).await.unwrap();
    assert_eq!(w.minutes.inputs.lock().unwrap()[0].user_note.as_deref(), Some("Pedir descuento"));
}

/// Caso real (16-nov-2026, app recién reinstalada): el permiso de micro/audio
/// del sistema estaba «sin decidir», `capture.start` no respondió y la reunión
/// quedó como «Lista» sin nada dentro y sin explicar por qué.
#[tokio::test]
async fn a_meeting_without_any_recorded_audio_fails_with_a_clear_reason() {
    let w = world().await;
    let id = MeetingId::from_u128(9);
    w.store
        .create_meeting(NewMeeting { id, title: "Grabación".into(), platform: Platform::Other, app_bundle_id: None, started_at: T0, language: "es".into() })
        .await
        .unwrap();
    let cfg = SegmentWriterConfig { layout: w.layout.clone(), meeting_id: id, format: AudioFormat::WORKING, segment_seconds: 30, flush_every_ms: 1_000 };
    SegmentWriter::create(cfg, T0).unwrap().finish(StopReason::Error, T0).unwrap();
    let sink = Sink(Mutex::new(vec![]));
    let e = w.pipeline.run(id, &ProcessingStep::ORDER, &sink).await.unwrap_err();
    assert!(e.to_string().contains("No se grabó audio"), "{e}");
    assert!(w.minutes.inputs.lock().unwrap().is_empty());
}

/// «Reintentar» empieza en `Transcribing` (el audio ya está unido): una
/// reunión vacía pasaba entonces a «Lista» sin nada dentro y sin motivo.
#[tokio::test]
async fn retrying_a_meeting_without_audio_fails_with_the_same_reason() {
    let w = world().await;
    let id = MeetingId::from_u128(10);
    w.store
        .create_meeting(NewMeeting { id, title: "Grabación".into(), platform: Platform::Other, app_bundle_id: None, started_at: T0, language: "es".into() })
        .await
        .unwrap();
    let cfg = SegmentWriterConfig { layout: w.layout.clone(), meeting_id: id, format: AudioFormat::WORKING, segment_seconds: 30, flush_every_ms: 1_000 };
    SegmentWriter::create(cfg, T0).unwrap().finish(StopReason::Error, T0).unwrap();
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(id, &ProcessingStep::ORDER, &sink).await.unwrap_err();
    // Segundo intento, con los pasos que manda el botón de la UI.
    let e = w.pipeline.run(id, &ProcessingStep::ORDER[1..], &sink).await.unwrap_err();
    assert!(e.to_string().contains("No se grabó audio"), "{e}");
}

/// Con transcripción en vivo, al parar solo se transcribe la cola que faltaba.
#[tokio::test]
async fn live_transcription_means_only_the_tail_is_transcribed_after_stopping() {
    struct Recorder(Mutex<Vec<std::path::PathBuf>>);
    #[async_trait]
    impl Transcriber for Recorder {
        async fn transcribe(&self, r: TranscribeRequest, _p: Option<ProgressFn>) -> PortResult<Vec<AsrSegment>> {
            self.0.lock().unwrap().push(r.wav_path.clone());
            Ok(vec![AsrSegment { start_ms: 0, end_ms: 1_500, text: "y cerramos".into(), confidence: None, words: vec![] }])
        }
    }
    let mut w = world().await;
    let rec = Arc::new(Recorder(Mutex::new(vec![])));
    w.pipeline.transcriber = rec.clone();
    for track in Track::ALL {
        let state = acta_audio::live::LiveAsrState {
            covered_ms: 4_000,
            segments: vec![AsrSegment { start_ms: 500, end_ms: 3_800, text: format!("en vivo {}", track.as_str()), confidence: None, words: vec![] }],
        };
        std::fs::create_dir_all(w.layout.live_asr_path(w.id, track).parent().unwrap()).unwrap();
        std::fs::write(w.layout.live_asr_path(w.id, track), serde_json::to_vec(&state).unwrap()).unwrap();
    }
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();
    let calls = rec.0.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "una cola por pista: {calls:?}");
    assert!(calls.iter().all(|p| p.to_string_lossy().contains("/live/win-")), "{calls:?}");
    let mic: Vec<AsrSegment> = serde_json::from_slice(&std::fs::read(w.layout.asr_cache_json(w.id, Track::Mic)).unwrap()).unwrap();
    assert_eq!(mic.iter().map(|s| (s.start_ms, s.text.as_str())).collect::<Vec<_>>(), vec![(500, "en vivo mic"), (4_000, "y cerramos")]);
    // «Reintentar» transcribe la pista entera otra vez.
    rec.0.lock().unwrap().clear();
    w.pipeline.run(w.id, &[ProcessingStep::Transcribing], &sink).await.unwrap();
    assert!(rec.0.lock().unwrap().iter().all(|p| !p.to_string_lossy().contains("/live/")));
}

/// El idioma de Ajustes dirige el ASR (pista) y el acta: `auto` deja que el
/// transcriptor lo detecte y el acta se escribe en el idioma de la reunión.
#[tokio::test]
async fn the_meeting_language_setting_drives_the_asr_hint_and_the_minutes_language() {
    struct LangAsr(Arc<Mutex<Vec<Option<String>>>>);
    #[async_trait]
    impl Transcriber for LangAsr {
        async fn transcribe(&self, r: TranscribeRequest, p: Option<ProgressFn>) -> PortResult<Vec<AsrSegment>> {
            self.0.lock().unwrap().push(r.language.clone());
            FakeAsr.transcribe(r, p).await
        }
    }
    for (setting, hint, minutes_language) in [("auto", None, "auto"), ("es-ES", Some("es"), "es"), ("", None, "auto"), ("ja", None, "ja")] {
        let w = world().await;
        let seen = Arc::new(Mutex::new(vec![]));
        let mut settings = Settings::default();
        settings.transcription.language = setting.into();
        // Sin nombre en Ajustes (instalación nueva): no rompe la fusión.
        assert_eq!(settings.user.display_name, "");
        let pipeline = DefaultPipeline { settings, transcriber: Arc::new(LangAsr(seen.clone())), ..w.pipeline };
        pipeline.run(w.id, &ProcessingStep::ORDER, &Sink(Mutex::new(vec![]))).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert!(!seen.is_empty() && seen.iter().all(|l| l.as_deref() == hint), "{setting}: {seen:?}");
        assert_eq!(w.minutes.inputs.lock().unwrap()[0].language, minutes_language, "{setting}");
    }
}

/// Namer falso: guarda lo que recibe y responde con `reply` (o falla).
struct FakeNamer {
    reply: Result<Vec<(String, String)>, String>,
    inputs: Mutex<Vec<SpeakerNamingInput>>,
}
#[async_trait]
impl SpeakerNamer for FakeNamer {
    async fn name_speakers(&self, input: SpeakerNamingInput) -> PortResult<Vec<SpeakerNameGuess>> {
        self.inputs.lock().unwrap().push(input.clone());
        let reply = self.reply.clone().map_err(PortError::Unavailable)?;
        Ok(reply
            .into_iter()
            .filter_map(|(label, name)| {
                let s = input.speakers.iter().find(|s| s.label == label)?;
                Some(SpeakerNameGuess { speaker_id: s.id, name, evidence: "«Gracias, Marta»".into() })
            })
            .collect())
    }
}

/// Caso real (19-nov-2026): en las 9 reuniones procesadas TODOS los
/// hablantes quedaron «Speaker N» aunque se oía «Hola, soy María» o
/// «gracias, Pepe»: sin huellas ni nombres de Teams no había otra fuente.
#[tokio::test]
async fn generic_speakers_get_the_name_said_in_the_conversation() {
    let mut w = world().await;
    let namer = Arc::new(FakeNamer { reply: Ok(vec![("Speaker 2".into(), "Marta".into())]), inputs: Mutex::new(vec![]) });
    w.pipeline.speaker_namer = Some(namer.clone());
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &Sink(Mutex::new(vec![]))).await.unwrap();

    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    let marta = t.speakers.iter().find(|s| s.label == "Speaker 2").unwrap();
    assert_eq!((marta.display_name.as_deref(), marta.name_source, marta.confirmed), (Some("Marta"), NameSource::Transcript, false));
    // Solo se pregunta por los que no tienen nombre (Luis ya viene de Teams).
    let input = namer.inputs.lock().unwrap()[0].clone();
    assert_eq!(input.targets, vec![marta.id]);
    assert!(input.participants.contains(&"Luis".to_string()), "{:?}", input.participants);
    assert!(input.self_names.contains(&"Edu Navajas".to_string()), "{:?}", input.self_names);
    assert_eq!(input.title, "Reunión de Teams");
    // El acta ya recibe el nombre.
    let m = w.minutes.inputs.lock().unwrap()[0].clone();
    assert!(m.speakers.iter().any(|s| s.shown_name() == "Marta"));
}

#[tokio::test]
async fn a_failing_speaker_namer_keeps_generic_names_and_does_not_fail_the_meeting() {
    let mut w = world().await;
    w.pipeline.speaker_namer = Some(Arc::new(FakeNamer { reply: Err("claude caído".into()), inputs: Mutex::new(vec![]) }));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &Sink(Mutex::new(vec![]))).await.unwrap();
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    assert!(t.speakers.iter().any(|s| s.label == "Speaker 2" && s.display_name.is_none()));
}

#[tokio::test]
async fn a_guessed_name_never_duplicates_another_speaker_or_replaces_a_confirmed_one() {
    let mut w = world().await;
    // El LLM propone «Luis» (ya es Speaker 1 por Teams): se descarta.
    let namer = Arc::new(FakeNamer { reply: Ok(vec![("Speaker 2".into(), "luis".into())]), inputs: Mutex::new(vec![]) });
    w.pipeline.speaker_namer = Some(namer.clone());
    let sink = Sink(Mutex::new(vec![]));
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &sink).await.unwrap();
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    let s2 = t.speakers.iter().find(|s| s.label == "Speaker 2").unwrap().clone();
    assert_eq!((s2.display_name.as_deref(), s2.name_source), (None, NameSource::Generic));

    // Confirmado por el usuario: reprocesar no lo vuelve a preguntar.
    rename_speaker(&w.layout, w.store.as_ref(), w.store.as_ref(), w.id, s2.id, "Marta".into(), false, T0).await.unwrap();
    namer.inputs.lock().unwrap().clear();
    w.pipeline.run(w.id, &[ProcessingStep::NamingSpeakers], &sink).await.unwrap();
    assert!(namer.inputs.lock().unwrap().is_empty(), "sin hablantes genéricos no se llama al LLM");
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    assert_eq!(t.speakers.iter().find(|s| s.id == s2.id).unwrap().display_name.as_deref(), Some("Marta"));
}


/// Limpiador falso: aplica `f` a cada texto (`None` = esa tanda falló) y guarda lo que recibe.
struct FakeCleaner {
    f: fn(&str) -> Option<String>,
    inputs: Mutex<Vec<CleanupInput>>,
}
impl FakeCleaner {
    fn new(f: fn(&str) -> Option<String>) -> Arc<Self> {
        Arc::new(Self { f, inputs: Mutex::new(vec![]) })
    }
    fn calls(&self) -> usize {
        self.inputs.lock().unwrap().len()
    }
}
#[async_trait]
impl TranscriptCleaner for FakeCleaner {
    async fn clean(&self, input: CleanupInput) -> Vec<Option<String>> {
        self.inputs.lock().unwrap().push(input.clone());
        input.texts.iter().map(|t| (self.f)(t)).collect()
    }
}

fn jira(t: &str) -> Option<String> {
    Some(t.replace("informe", "informe de Jira"))
}

fn texts(t: &Transcript) -> Vec<String> {
    t.utterances.iter().map(|u| u.text.clone()).collect()
}

async fn run_all(w: &World) {
    w.pipeline.run(w.id, &ProcessingStep::ORDER, &Sink(Mutex::new(vec![]))).await.unwrap();
}

#[tokio::test]
async fn llm_cleanup_rewrites_the_text_before_the_minutes_without_touching_anything_else() {
    let w = world().await;
    run_all(&w).await;
    let raw = w.store.get_transcript(w.id).await.unwrap().unwrap();

    let mut w = world().await;
    let cleaner = FakeCleaner::new(jira);
    w.pipeline.cleaner = Some(cleaner.clone());
    w.pipeline.settings.transcription.glossary = vec!["Jira".into()];
    run_all(&w).await;
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();

    assert!(texts(&t).contains(&"Yo me encargo del informe de Jira".to_string()), "{:?}", texts(&t));
    // Mismas intervenciones, mismos hablantes, mismos tiempos.
    assert_eq!(t.utterances.len(), raw.utterances.len());
    for (a, b) in t.utterances.iter().zip(&raw.utterances) {
        assert_eq!((a.idx, a.track, a.start, a.end), (b.idx, b.track, b.start, b.end));
    }
    assert_eq!(t.speakers.len(), raw.speakers.len());
    // El acta se genera con el texto limpio.
    let m = w.minutes.inputs.lock().unwrap()[0].clone();
    assert!(m.utterances.iter().any(|u| u.text.ends_with("informe de Jira")));
    // Recibe el glosario y el idioma de la reunión.
    let input = cleaner.inputs.lock().unwrap()[0].clone();
    assert_eq!(input.glossary, vec!["Jira".to_string()]);
    assert_eq!(input.language, "auto");
    assert_eq!(input.texts.len(), raw.utterances.len());
    // La caché del ASR sigue cruda.
    let asr: Vec<AsrSegment> = serde_json::from_slice(&std::fs::read(w.layout.asr_cache_json(w.id, Track::System)).unwrap()).unwrap();
    assert!(asr.iter().all(|s| !s.text.contains("Jira")));
}

#[tokio::test]
async fn reprocessing_reuses_the_cleanup_cache_and_asks_the_llm_only_for_what_is_new() {
    let mut w = world().await;
    let cleaner = FakeCleaner::new(jira);
    w.pipeline.cleaner = Some(cleaner.clone());
    run_all(&w).await;
    assert_eq!(cleaner.calls(), 1);
    assert!(w.layout.cleanup_cache_json(w.id).exists());

    w.pipeline.run(w.id, &[ProcessingStep::NamingSpeakers], &Sink(Mutex::new(vec![]))).await.unwrap();
    assert_eq!(cleaner.calls(), 1, "mismo texto, mismo glosario: todo sale de la caché");
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    assert!(texts(&t).iter().any(|x| x.ends_with("informe de Jira")));
}

#[tokio::test]
async fn a_new_glossary_invalidates_the_cleanup_cache() {
    let mut w = world().await;
    let cleaner = FakeCleaner::new(jira);
    w.pipeline.cleaner = Some(cleaner.clone());
    run_all(&w).await;
    w.pipeline.settings.transcription.glossary = vec!["Terraform".into()];
    w.pipeline.run(w.id, &[ProcessingStep::NamingSpeakers], &Sink(Mutex::new(vec![]))).await.unwrap();
    assert_eq!(cleaner.calls(), 2);
}

#[tokio::test]
async fn a_failed_batch_keeps_the_raw_text_and_is_retried_next_time() {
    let mut w = world().await;
    let cleaner = FakeCleaner::new(|_| None);
    w.pipeline.cleaner = Some(cleaner.clone());
    run_all(&w).await; // no falla la reunión
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    assert!(texts(&t).contains(&"Yo me encargo del informe".to_string()));
    // Lo que falló NO se cachea: el siguiente procesado lo vuelve a pedir.
    w.pipeline.run(w.id, &[ProcessingStep::NamingSpeakers], &Sink(Mutex::new(vec![]))).await.unwrap();
    assert_eq!(cleaner.calls(), 2);
}

#[tokio::test]
async fn a_cleaner_that_returns_the_wrong_number_of_texts_is_ignored() {
    struct Broken;
    #[async_trait]
    impl TranscriptCleaner for Broken {
        async fn clean(&self, _input: CleanupInput) -> Vec<Option<String>> {
            vec![Some("solo uno".into())]
        }
    }
    let mut w = world().await;
    w.pipeline.cleaner = Some(Arc::new(Broken));
    run_all(&w).await;
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    assert!(texts(&t).iter().all(|x| x != "solo uno"), "{:?}", texts(&t));
    assert!(texts(&t).contains(&"Perfecto, gracias".to_string()));
}

#[tokio::test]
async fn the_setting_turns_the_cleanup_off() {
    let mut w = world().await;
    let cleaner = FakeCleaner::new(jira);
    w.pipeline.cleaner = Some(cleaner.clone());
    w.pipeline.settings.transcription.llm_cleanup = false;
    run_all(&w).await;
    assert_eq!(cleaner.calls(), 0);
    assert!(!w.layout.cleanup_cache_json(w.id).exists());
    let t = w.store.get_transcript(w.id).await.unwrap().unwrap();
    assert!(texts(&t).iter().all(|x| !x.contains("Jira")));
}
