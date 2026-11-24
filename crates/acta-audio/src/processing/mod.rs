//! `DefaultPipeline`: impl de `acta_core::processing::ProcessingPipeline`.
//! Pasos y reglas de fusión: docs/01-arquitectura.md §6.
//!
//! Cada paso deja su resultado en disco o en el store, así que se puede
//! reprocesar un paso suelto (`meeting_reprocess`):
//!
//! | paso | lee | escribe |
//! |---|---|---|
//! | `concatenating` | segmentos + manifiesto, `live/screen.jsonl` | `audio/{mic,system,mix}.wav`, duración, tramos de pantalla |
//! | `transcribing` | `audio/<track>.wav` | `transcript/segments-<track>.json` |
//! | `diarizing` | `audio/system.wav` | `transcript/diarization-result.json` |
//! | `namingSpeakers` | cachés anteriores + `live/speakers.jsonl` + huellas + LLM (limpieza del texto con el glosario, `transcript/cleanup.json`; nombres dichos en la reunión) | transcript en el store |
//! | `summarizing` | transcript + capturas | acta en el store + `llm/minutes.json` |
//! | `indexing` | transcript | chunks, FTS, vectores |
//! | `pruningCaptures` | `llm/minutes.json` | candidatas promovidas a `llmPick` o borradas |
//!
//! Un fallo del resumen (LLM caído) NO impide indexar: el transcript queda
//! buscable y la reunión acaba en `failed` con el error para reintentar.

pub mod captures;
pub mod energy;
pub mod glossary;
pub mod merge;
pub mod voiceprints;

use crate::concat::{finalize_audio, remove_segments};
use crate::supervisor::files::read_jsonl;
use crate::writer::read_manifest;
use acta_core::detection::SpeakerSignal;
use acta_core::ids::MeetingId;
use acta_core::layout::DataLayout;
use acta_core::library::{MeetingPatch, MeetingStore, SearchIndex, VoiceprintStore};
use acta_core::minutes::{CaptureRef, MinutesGenerator, MinutesInput, MinutesOutput};
use acta_core::processing::{ProcessingPipeline, ProcessingStep, ProgressSink};
use acta_core::recording::{SessionManifest, Track};
use acta_core::screen::{CaptureReason, ScreenEvent, ScreenShare};
use acta_core::settings::Settings;
use acta_core::time::MediaMs;
use acta_core::transcription::{
    AsrSegment, CleanupInput, DiarizationResult, DiarizeOptions, Diarizer, NameSource, SpeakerNamer, SpeakerNamingInput,
    TranscribeRequest, Transcriber, TranscriptCleaner, Transcript,
};
use acta_core::{PortError, PortResult};
use async_trait::async_trait;
use merge::{ActiveSpan, MergeInput, merge_transcript};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

pub struct DefaultPipeline {
    pub layout: DataLayout,
    pub settings: Settings,
    pub store: Arc<dyn MeetingStore>,
    pub voiceprints: Arc<dyn VoiceprintStore>,
    pub search: Arc<dyn SearchIndex>,
    pub transcriber: Arc<dyn Transcriber>,
    pub diarizer: Arc<dyn Diarizer>,
    pub minutes: Arc<dyn MinutesGenerator>,
    /// Nombres deducidos de la conversación para los "Speaker N" (LLM).
    /// `None` = sin proveedor: se quedan genéricos.
    pub speaker_namer: Option<Arc<dyn SpeakerNamer>>,
    /// Limpieza del texto con el LLM (jerga mal oída, puntuación). `None` = sin
    /// proveedor: el texto se queda como lo dejó el ASR + glosario.
    pub cleaner: Option<Arc<dyn TranscriptCleaner>>,
}

/// Caché de la limpieza LLM: texto tal como sale del merge → texto limpio.
/// `fingerprint` agrupa todo lo que cambia el resultado (reglas del prompt,
/// idioma, glosario): si no coincide, la caché entera no vale.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct CleanupCache {
    fingerprint: String,
    entries: HashMap<String, String>,
}

/// Duración media del manifiesto (la pista más larga).
pub fn manifest_duration(m: &SessionManifest) -> MediaMs {
    let rate = u64::from(m.format.sample_rate.max(1));
    let samples = m
        .tracks
        .iter()
        .map(|t| t.segments.iter().map(|s| s.samples).sum::<u64>())
        .max()
        .unwrap_or(0);
    MediaMs(samples * 1000 / rate)
}

/// Nombre del paso para mensajes al usuario.
pub fn step_label(step: ProcessingStep) -> &'static str {
    match step {
        ProcessingStep::Concatenating => "Unir el audio",
        ProcessingStep::Transcribing => "Transcribir",
        ProcessingStep::Diarizing => "Separar hablantes",
        ProcessingStep::NamingSpeakers => "Nombrar hablantes",
        ProcessingStep::Summarizing => "Resumen",
        ProcessingStep::Indexing => "Indexar para búsqueda",
        ProcessingStep::PruningCaptures => "Capturas",
    }
}

fn step_error(step: ProcessingStep, e: PortError) -> PortError {
    let detail = match e {
        PortError::Internal(m) | PortError::Unavailable(m) | PortError::Io(m) | PortError::InvalidResponse(m) => m,
        other => other.to_string(),
    };
    PortError::Internal(format!("Falló el paso «{}»: {detail}", step_label(step)))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> PortResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::fileio::write_atomic(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

/// Sin un solo sample en ninguna pista no hay reunión que procesar.
fn ensure_recorded_audio(manifest: &acta_core::recording::SessionManifest) -> PortResult<()> {
    if manifest.tracks.iter().all(|t| t.segments.iter().all(|s| s.samples == 0)) {
        return Err(PortError::Unavailable(
            "No se grabó audio: la captura no llegó a empezar. Revisa en Ajustes › Permisos el Micrófono y \
             el Audio del sistema (tras reinstalar acta, macOS vuelve a pedirlos)."
                .into(),
        ));
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> PortResult<T> + Send + 'static) -> PortResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| PortError::Internal(format!("join: {e}")))?
}

/// Pares inicio/fin de `screen.jsonl` → tramos en tiempo media.
pub fn screen_shares(events: &[ScreenEvent], manifest: &SessionManifest) -> Vec<ScreenShare> {
    let mut out: Vec<ScreenShare> = Vec::new();
    for ev in events {
        match ev {
            ScreenEvent::ShareStarted { presenter, at } => {
                if out.last().is_some_and(|s| s.end.is_none()) {
                    continue;
                }
                if let Some(start) = manifest.wall_to_media(*at) {
                    out.push(ScreenShare { start, end: None, presenter: presenter.clone() });
                }
            }
            ScreenEvent::ShareEnded { at } => {
                if let Some(open) = out.last_mut().filter(|s| s.end.is_none()) {
                    open.end = manifest.wall_to_media(*at).or(Some(manifest_duration(manifest)));
                }
            }
            ScreenEvent::FrameSaved { .. } => {}
        }
    }
    out
}

/// Señales `Active` → tramos en tiempo media (cada uno dura hasta la siguiente).
pub fn active_spans(signals: &[SpeakerSignal], manifest: &SessionManifest) -> Vec<ActiveSpan> {
    let actives: Vec<(acta_core::time::UnixMs, &Vec<String>)> = signals
        .iter()
        .filter_map(|s| match s {
            SpeakerSignal::Active { names, at } => Some((*at, names)),
            SpeakerSignal::Roster { .. } => None,
        })
        .collect();
    let end_of_media = manifest_duration(manifest);
    let mut out = Vec::new();
    for (i, (at, names)) in actives.iter().enumerate() {
        if names.is_empty() {
            continue;
        }
        let Some(start) = manifest.wall_to_media(*at) else { continue };
        let end = actives
            .get(i + 1)
            .and_then(|(next, _)| manifest.wall_to_media(*next))
            .unwrap_or(end_of_media);
        if end > start {
            out.push(ActiveSpan { start, end, names: (*names).clone() });
        }
    }
    out
}

impl DefaultPipeline {
    async fn concatenate(&self, id: MeetingId) -> PortResult<()> {
        let layout = self.layout.clone();
        let delete = self.settings.recording.delete_segments_after_concat;
        let manifest = blocking(move || {
            let m = finalize_audio(&layout, id)?;
            if delete {
                remove_segments(&layout, &m)?;
            }
            Ok(m)
        })
        .await?;
        let duration = manifest_duration(&manifest);
        let patch = MeetingPatch {
            duration: Some(duration),
            ..MeetingPatch::default()
        };
        self.store.patch_meeting(id, patch).await?;
        ensure_recorded_audio(&manifest)?;
        let events: Vec<ScreenEvent> = read_jsonl(&self.layout.live_screen_path(id));
        self.store.save_screen_shares(id, screen_shares(&events, &manifest)).await
    }

    async fn transcribe(&self, id: MeetingId, progress: &dyn ProgressSink) -> PortResult<()> {
        if !self.layout.track_wav(id, Track::Mic).exists() && !self.layout.track_wav(id, Track::System).exists() {
            self.concatenate(id).await?;
        }
        // «Reintentar» entra por aquí con el audio ya unido: una reunión vacía
        // debe volver a fallar con su motivo, no quedar «Lista» sin nada.
        let layout = self.layout.clone();
        if let Ok(manifest) = blocking(move || crate::writer::read_manifest(&layout, id)).await {
            ensure_recorded_audio(&manifest)?;
        }
        // `auto` (o un idioma que el transcriptor no conoce): que lo detecte él.
        let language = acta_core::settings::language::asr_hint(&self.settings.transcription.language);
        for (i, track) in Track::ALL.into_iter().enumerate() {
            let wav = self.layout.track_wav(id, track);
            // Primera vez tras grabar con transcripción en vivo: solo falta la
            // cola. «Reintentar» (ya hay caché) transcribe la pista entera.
            let live = if wav.exists() && !self.layout.asr_cache_json(id, track).exists() {
                crate::live::finish_track(&self.layout, id, track, self.transcriber.as_ref(), language.clone()).await?
            } else {
                None
            };
            let segments = if let Some(segments) = live {
                segments
            } else if wav.exists() {
                self.transcriber
                    .transcribe(
                        TranscribeRequest {
                            wav_path: wav,
                            track,
                            language: language.clone(),
                        },
                        None,
                    )
                    .await?
            } else {
                Vec::new()
            };
            write_json(&self.layout.asr_cache_json(id, track), &segments)?;
            if i == 0 {
                progress.progress(id, ProcessingStep::Transcribing, 0.5);
            }
        }
        Ok(())
    }

    async fn diarize(&self, id: MeetingId) -> PortResult<()> {
        let wav = self.layout.track_wav(id, Track::System);
        let result = if self.settings.transcription.diarization && wav.exists() {
            self.diarizer.diarize(&wav, DiarizeOptions::default(), None).await?
        } else {
            DiarizationResult {
                turns: vec![],
                speakers: vec![],
                embedding_model: String::new(),
            }
        };
        write_json(&self.layout.diarization_cache_json(id), &result)
    }

    /// Envolventes de energía de mic y system para separar voz propia de bleed.
    /// Sin una de las pistas (o si falla la lectura) → `None` y la fusión
    /// resuelve los duplicados a favor del sistema.
    async fn track_levels(&self, id: MeetingId) -> Option<energy::TrackLevels> {
        let mic = self.layout.track_wav(id, Track::Mic);
        let system = self.layout.track_wav(id, Track::System);
        if !mic.exists() || !system.exists() {
            return None;
        }
        let r = blocking(move || {
            Ok(energy::TrackLevels { mic: energy::Envelope::from_wav(&mic)?, system: energy::Envelope::from_wav(&system)? })
        })
        .await;
        r.map_err(|e| tracing::warn!("envolventes de energía no disponibles: {e}")).ok()
    }

    async fn name_speakers(&self, id: MeetingId) -> PortResult<()> {
        let mut mic: Vec<AsrSegment> = read_json(&self.layout.asr_cache_json(id, Track::Mic)).unwrap_or_default();
        let mut system: Vec<AsrSegment> = read_json(&self.layout.asr_cache_json(id, Track::System)).unwrap_or_default();
        // La caché ASR queda cruda: el glosario se aplica aquí, así que
        // reprocesar el paso con un glosario nuevo basta.
        glossary::apply(&mut mic, &self.settings.transcription.glossary);
        glossary::apply(&mut system, &self.settings.transcription.glossary);
        let diarization: Option<DiarizationResult> = read_json(&self.layout.diarization_cache_json(id));
        let signals: Vec<SpeakerSignal> = read_jsonl(&self.layout.live_speakers_path(id));
        let active = read_manifest(&self.layout, id)
            .map(|m| active_spans(&signals, &m))
            .unwrap_or_default();
        let participants: Vec<String> = signals
            .iter()
            .filter_map(|s| match s {
                SpeakerSignal::Roster { participants, .. } => Some(participants),
                SpeakerSignal::Active { .. } => None,
            })
            .flatten()
            .filter(|p| !p.is_self && !p.name.trim().is_empty())
            .map(|p| p.name.trim().to_owned())
            .fold(Vec::new(), |mut acc, n| {
                if !acc.contains(&n) {
                    acc.push(n);
                }
                acc
            });
        let mut self_names: Vec<String> = signals
            .iter()
            .filter_map(|s| match s {
                SpeakerSignal::Roster { participants, .. } => Some(participants),
                SpeakerSignal::Active { .. } => None,
            })
            .flatten()
            .filter(|p| p.is_self)
            .map(|p| p.name.clone())
            .collect();
        self_names.push(self.settings.user.display_name.clone());
        self_names.extend(self.settings.user.aliases.iter().cloned());
        // Sin nombre en Ajustes no hay nada que comparar.
        self_names.retain(|n| !n.trim().is_empty());

        let mut voiceprints = HashMap::new();
        if let Some(d) = &diarization {
            let min = self.settings.transcription.voiceprint_min_similarity_pct as f32 / 100.0;
            for s in d.speakers.iter().filter(|s| !s.embedding.is_empty()) {
                match self.voiceprints.nearest_voiceprint(&s.embedding, &d.embedding_model, min).await {
                    Ok(Some((vp, _))) => {
                        voiceprints.insert(s.label.clone(), (vp.id, vp.display_name));
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("búsqueda de huella fallida: {e}"),
                }
            }
        }
        let previous = self.store.get_transcript(id).await?;
        let levels = self.track_levels(id).await;
        let mut transcript = merge_transcript(
            id,
            MergeInput {
                mic,
                system,
                diarization,
                active,
                self_names: self_names.clone(),
                voiceprints,
                previous,
                levels,
            },
        );
        self.clean_with_llm(&mut transcript).await;
        self.names_from_conversation(&mut transcript, self_names, participants).await;
        self.store.save_transcript(transcript).await
    }

    /// Pasada del LLM sobre el texto de cada intervención (jerga mal oída con el
    /// glosario, puntuación, mayúsculas). Corre sobre el transcript ya fusionado
    /// —solo lo que de verdad se queda, no el bleed descartado— y ANTES del acta.
    ///
    /// Solo cambia `text`: ni hablantes, ni tiempos, ni número de intervenciones.
    /// Nunca hace fallar el paso: lo que no se limpia se queda crudo y no se
    /// cachea (el siguiente procesado lo reintenta). Lo limpiado se cachea por
    /// texto, así que reprocesar solo pide al LLM lo nuevo.
    async fn clean_with_llm(&self, transcript: &mut Transcript) {
        let Some(cleaner) = &self.cleaner else { return };
        if !self.settings.transcription.llm_cleanup || transcript.utterances.is_empty() {
            return;
        }
        let language = acta_core::settings::language::normalize(&self.settings.transcription.language);
        let mut glossary: Vec<String> =
            self.settings.transcription.glossary.iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect();
        glossary.sort();
        glossary.dedup();
        let fingerprint = format!("{}|{language}|{}", cleaner.version(), glossary.join(","));

        let path = self.layout.cleanup_cache_json(transcript.meeting_id);
        let mut cache = read_json::<CleanupCache>(&path).filter(|c| c.fingerprint == fingerprint).unwrap_or_default();
        cache.fingerprint = fingerprint;

        let mut missing: Vec<String> = Vec::new();
        for u in &transcript.utterances {
            if !cache.entries.contains_key(&u.text) && !missing.contains(&u.text) {
                missing.push(u.text.clone());
            }
        }
        let mut dirty = false;
        if !missing.is_empty() {
            let cleaned = cleaner.clean(CleanupInput { texts: missing.clone(), glossary, language }).await;
            if cleaned.len() == missing.len() {
                for (raw, c) in missing.into_iter().zip(cleaned) {
                    if let Some(c) = c {
                        cache.entries.insert(raw, c);
                        dirty = true;
                    }
                }
            } else {
                tracing::warn!(pedidos = missing.len(), devueltos = cleaned.len(), "limpieza del transcript: número de textos distinto; se ignora");
            }
        }
        // La caché solo guarda lo que este transcript usa.
        let used: std::collections::HashSet<&str> = transcript.utterances.iter().map(|u| u.text.as_str()).collect();
        let before = cache.entries.len();
        cache.entries.retain(|raw, _| used.contains(raw.as_str()));
        if dirty || cache.entries.len() != before {
            if let Err(e) = write_json(&path, &cache) {
                tracing::warn!("no se pudo guardar la caché de limpieza: {e}");
            }
        }
        for u in &mut transcript.utterances {
            if let Some(c) = cache.entries.get(&u.text) {
                u.text.clone_from(c);
            }
        }
    }

    /// Último recurso para los "Speaker N": el nombre que se dice en la
    /// reunión. Nunca hace fallar el paso (se quedan genéricos) ni pisa un
    /// nombre que ya tenga otro hablante.
    async fn names_from_conversation(&self, transcript: &mut Transcript, self_names: Vec<String>, participants: Vec<String>) {
        let Some(namer) = &self.speaker_namer else { return };
        let targets: Vec<_> = transcript
            .speakers
            .iter()
            .filter(|s| !s.is_me && !s.confirmed && s.name_source == NameSource::Generic)
            .map(|s| s.id)
            .collect();
        if targets.is_empty() || transcript.utterances.is_empty() {
            return;
        }
        let title = match self.store.get_meeting(transcript.meeting_id).await {
            Ok(Some(m)) => m.title,
            _ => String::new(),
        };
        let input = SpeakerNamingInput {
            title,
            me_name: self.settings.user.display_name.clone(),
            self_names: self_names.clone(),
            participants,
            speakers: transcript.speakers.clone(),
            utterances: transcript.utterances.clone(),
            targets: targets.clone(),
        };
        let guesses = match namer.name_speakers(input).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(meeting_id = %transcript.meeting_id, "no se pudieron deducir nombres de la conversación: {e}");
                return;
            }
        };
        let norm = |s: &str| s.trim().to_lowercase();
        for g in guesses {
            let name = g.name.trim().to_owned();
            if name.is_empty() || !targets.contains(&g.speaker_id) || self_names.iter().any(|n| norm(n) == norm(&name)) {
                continue;
            }
            let taken = transcript
                .speakers
                .iter()
                .any(|s| s.id != g.speaker_id && s.display_name.as_deref().is_some_and(|d| norm(d) == norm(&name)));
            if taken {
                continue;
            }
            if let Some(s) = transcript.speakers.iter_mut().find(|s| s.id == g.speaker_id && s.name_source == NameSource::Generic) {
                tracing::info!(meeting_id = %transcript.meeting_id, label = %s.label, evidence = %g.evidence, "nombre deducido de la conversación");
                s.display_name = Some(name);
                s.name_source = NameSource::Transcript;
            }
        }
    }

    async fn summarize(&self, id: MeetingId) -> PortResult<bool> {
        let meeting = self
            .store
            .get_meeting(id)
            .await?
            .ok_or_else(|| PortError::NotFound(format!("reunión {id}")))?;
        let transcript = self.store.get_transcript(id).await?.unwrap_or(acta_core::transcription::Transcript {
            meeting_id: id,
            speakers: vec![],
            utterances: vec![],
        });
        if transcript.utterances.is_empty() {
            tracing::info!(%id, "sin transcript: no hay nada que resumir");
            return Ok(false);
        }
        let mut caps: Vec<_> = self
            .store
            .list_captures(id)
            .await?
            .into_iter()
            .filter(|c| c.reason != CaptureReason::Candidate)
            .collect();
        caps.sort_by_key(|c| c.at);
        let refs = caps
            .iter()
            .enumerate()
            .map(|(i, c)| CaptureRef {
                ref_id: format!("c{}", i + 1),
                capture_id: c.id,
                at: c.at,
                reason: c.reason,
            })
            .collect();
        let input = MinutesInput {
            meeting_id: id,
            title: meeting.title.clone(),
            platform: meeting.platform,
            started_at: meeting.started_at,
            duration: meeting.duration,
            me: self.settings.user.clone(),
            speakers: transcript.speakers,
            utterances: transcript.utterances,
            captures: refs,
            language: acta_core::settings::language::normalize(&self.settings.transcription.language),
            user_note: meeting.user_note.clone(),
        };
        let output: MinutesOutput = self.minutes.generate(input).await?;
        write_json(&self.layout.minutes_json(id), &output)?;
        self.store.save_minutes(output.clone()).await?;
        // Capturas que el LLM no colocó en ningún tema: por tiempo.
        for mut c in self.store.list_captures(id).await? {
            if c.reason == CaptureReason::Candidate || c.topic_path.is_some() {
                continue;
            }
            if let Some(p) = captures::topic_path_for(&output.escaleta, c.at) {
                c.topic_path = Some(p);
                self.store.update_capture(c).await?;
            }
        }
        Ok(true)
    }

    async fn prune(&self, id: MeetingId) -> PortResult<()> {
        let Some(output) = read_json::<MinutesOutput>(&self.layout.minutes_json(id)) else {
            return Ok(());
        };
        captures::prune_candidates(
            &self.layout,
            self.store.as_ref(),
            id,
            &output,
            self.settings.screen.prune_candidates,
        )
        .await
    }
}

#[async_trait]
impl ProcessingPipeline for DefaultPipeline {
    async fn run(&self, meeting_id: MeetingId, steps: &[ProcessingStep], progress: &dyn ProgressSink) -> PortResult<()> {
        // Contrato de `ReprocessRequest`: vacío = todos desde `Transcribing`.
        let mut wanted: BTreeSet<ProcessingStep> = if steps.is_empty() {
            ProcessingStep::ORDER.iter().copied().filter(|s| *s != ProcessingStep::Concatenating).collect()
        } else {
            steps.iter().copied().collect()
        };
        if wanted.contains(&ProcessingStep::Transcribing) || wanted.contains(&ProcessingStep::Diarizing) {
            wanted.insert(ProcessingStep::NamingSpeakers);
        }
        let mut deferred: Option<PortError> = None;
        let mut summarized = true;
        for step in ProcessingStep::ORDER {
            if !wanted.contains(&step) {
                continue;
            }
            progress.progress(meeting_id, step, 0.0);
            let started = std::time::Instant::now();
            let result = match step {
                ProcessingStep::Concatenating => self.concatenate(meeting_id).await,
                ProcessingStep::Transcribing => self.transcribe(meeting_id, progress).await,
                ProcessingStep::Diarizing => self.diarize(meeting_id).await,
                ProcessingStep::NamingSpeakers => self.name_speakers(meeting_id).await,
                ProcessingStep::Summarizing => self.summarize(meeting_id).await.map(|done| summarized = done),
                ProcessingStep::Indexing => self.search.index_meeting(meeting_id).await,
                ProcessingStep::PruningCaptures if summarized && deferred.is_none() => self.prune(meeting_id).await,
                ProcessingStep::PruningCaptures => Ok(()),
            };
            tracing::info!(%meeting_id, step = step_label(step), ms = started.elapsed().as_millis() as u64, ok = result.is_ok(), "paso del procesado");
            match result {
                Ok(()) => progress.progress(meeting_id, step, 1.0),
                Err(e) if step == ProcessingStep::Summarizing => {
                    tracing::error!(%meeting_id, "resumen fallido, se sigue indexando: {e}");
                    deferred = Some(step_error(step, e));
                }
                Err(e) => return Err(step_error(step, e)),
            }
        }
        deferred.map_or(Ok(()), Err)
    }
}
