#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Especificación del supervisor de sesión con fakes de todos los puertos.

mod support;

use acta_audio::supervisor::{SupervisorConfig, SupervisorEvent, SupervisorHandle, spawn_with};
use acta_audio::writer::{SegmentWriter, SegmentWriterConfig, read_manifest};
use acta_core::detection::{Confidence, DetectionEvent, Participant, SpeakerSignal};
use acta_core::library::MeetingStatus;
use acta_core::processing::ProcessingStep;
use acta_core::recording::*;
use acta_core::screen::{CaptureReason, ScreenEvent};
use acta_core::settings::Settings;
use acta_core::time::UnixMs;
use acta_core::transcription::LivePartial;
use std::sync::Arc;
use std::time::Duration;
use support::*;
use tokio::sync::Notify;

fn config() -> SupervisorConfig {
    SupervisorConfig {
        tick: Duration::from_millis(30),
        stop_drain: Duration::from_millis(500),
    }
}

async fn start(rig: &Rig, settings: Settings) -> SupervisorHandle {
    spawn_with(rig.ports(), settings, config()).await.unwrap()
}

async fn wait_elapsed(h: &SupervisorHandle, ms: u64) {
    for _ in 0..250 {
        if h.snapshot().await.unwrap().elapsed_ms >= ms {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no se grabaron {ms} ms");
}

fn recording_id(s: &SessionState) -> acta_core::ids::MeetingId {
    match s {
        SessionState::Recording(r) => r.meeting_id,
        SessionState::Paused { recording, .. } => recording.meeting_id,
        other => panic!("no graba: {other:?}"),
    }
}

async fn detect_and_accept(rig: &Rig, h: &SupervisorHandle, rx: &mut tokio::sync::broadcast::Receiver<SupervisorEvent>) -> acta_core::ids::MeetingId {
    rig.detector.push(DetectionEvent::MeetingStarted { app: teams_app(), confidence: Confidence::High, at: T0 });
    let snap = wait_state(rx, "detected").await;
    let SessionState::Detected { prompt, .. } = snap.state else { unreachable!() };
    let snap = h
        .send(SessionEvent::PromptAnswered { prompt_id: prompt.id, answer: PromptAnswer::Accept, at: T0 })
        .await
        .unwrap();
    recording_id(&snap.state)
}

#[tokio::test]
async fn manual_recording_writes_both_tracks_then_processes_in_background() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let mut rx = h.subscribe();

    let snap = h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap();
    let id = recording_id(&snap.state);
    let m = rig.store.meeting(id).expect("reunión creada");
    assert_eq!(m.status, MeetingStatus::Recording);
    assert_eq!(m.title, "Grabación");
    let req = rig.audio.requests.lock().unwrap()[0].clone();
    assert!(req.system.pids.is_empty() && req.system.bundle_ids.is_empty(), "manual = tap global");

    rig.audio.frames(Track::Mic, 0, 20, 1000);
    rig.audio.frames(Track::System, 0, 20, 1000);
    wait_for(&mut rx, |e| matches!(e, SupervisorEvent::Levels(_))).await;
    wait_elapsed(&h, 2_000).await;

    let snap = h.send(SessionEvent::StopRequested { reason: StopReason::User, at: T0.plus_ms(2_000) }).await.unwrap();
    assert_eq!(snap.state.name(), "processing", "parar encola el procesado");
    let manifest = read_manifest(&rig.layout, id).unwrap();
    assert_eq!(manifest.status, ManifestStatus::Stopped);
    for t in &manifest.tracks {
        assert_eq!(t.segments.iter().map(|s| s.samples).sum::<u64>(), 32_000, "{:?}", t.track);
    }
    wait_for(&mut rx, |e| matches!(e, SupervisorEvent::MeetingUpdated(u) if u.status == MeetingStatus::Ready)).await;
    wait_state(&mut rx, "ready").await;
    let runs = rig.pipeline.runs.lock().unwrap().clone();
    assert_eq!(runs, vec![(id, ProcessingStep::ORDER.to_vec())]);
    let m = rig.store.meeting(id).unwrap();
    assert_eq!(m.status, MeetingStatus::Ready);
    assert_eq!(m.duration.0, 2_000);
    assert_eq!(rig.audio.calls(), vec!["start", "stop"]);
}

#[tokio::test]
async fn detection_asks_to_record_then_asks_to_stop_when_the_call_ends() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let mut rx = h.subscribe();
    let id = detect_and_accept(&rig, &h, &mut rx).await;

    let m = rig.store.meeting(id).unwrap();
    assert_eq!(m.title, "Daily");
    let req = rig.audio.requests.lock().unwrap()[0].clone();
    assert_eq!(req.system.pids, vec![42, 43]);
    assert_eq!(req.system.bundle_ids, vec!["com.microsoft.teams2".to_string()]);
    assert_eq!(rig.speakers.watched.lock().unwrap().clone(), vec!["com.microsoft.teams2".to_string()]);

    rig.detector.push(DetectionEvent::MeetingEnded { app: teams_app(), at: T0.plus_ms(5_000) });
    let snap = wait_for(&mut rx, |e| {
        matches!(e, SupervisorEvent::Snapshot(s) if matches!(&s.state, SessionState::Recording(r) if r.stop_prompt.is_some()))
    })
    .await;
    let SupervisorEvent::Snapshot(snap) = snap else { unreachable!() };
    let SessionState::Recording(rec) = snap.state else { unreachable!() };
    let prompt = rec.stop_prompt.unwrap();
    let snap = h
        .send(SessionEvent::PromptAnswered { prompt_id: prompt.id, answer: PromptAnswer::Accept, at: T0.plus_ms(6_000) })
        .await
        .unwrap();
    assert_eq!(snap.state.name(), "processing");
    assert_eq!(read_manifest(&rig.layout, id).unwrap().stop_reason, Some(StopReason::MeetingEndedConfirmed));
}

#[tokio::test]
async fn denylisted_apps_never_trigger() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.detection.denylist = vec!["com.microsoft.teams2".into()];
    let h = start(&rig, settings).await;
    let cfg = rig.detector.configs.lock().unwrap()[0].clone();
    assert!(cfg.rules.iter().all(|r| r.bundle_id != "com.microsoft.teams2"));
    rig.detector.push(DetectionEvent::MeetingStarted { app: teams_app(), confidence: Confidence::High, at: T0 });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(h.snapshot().await.unwrap().state, SessionState::Idle);
}

#[tokio::test]
async fn unanswered_start_prompt_expires() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.detection.start_prompt_timeout_s = 1;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();
    rig.detector.push(DetectionEvent::MeetingStarted { app: teams_app(), confidence: Confidence::High, at: T0 });
    wait_state(&mut rx, "detected").await;
    rig.clock.advance_ms(1_500);
    wait_state(&mut rx, "idle").await;
}

#[tokio::test]
async fn crash_during_recording_is_recovered_and_processed_on_startup() {
    let rig = Rig::new();
    let id = acta_core::ids::MeetingId::from_u128(77);
    {
        let cfg = SegmentWriterConfig {
            layout: rig.layout.clone(),
            meeting_id: id,
            format: AudioFormat::WORKING,
            segment_seconds: 1,
            flush_every_ms: 100,
        };
        let mut w = SegmentWriter::create(cfg, T0).unwrap();
        for n in 0..15 {
            w.push(&AudioFrame { track: Track::Mic, seq: n, pts_us: n * 100_000, sample_rate: 16_000, samples: vec![7; 1600] })
                .unwrap();
        }
        // Sin finish(): la app "muere" aquí.
    }
    let h = start(&rig, Settings::default()).await;
    let mut rx = h.subscribe();
    eventually(|| rig.pipeline.runs.lock().unwrap().iter().any(|(m, _)| *m == id)).await;
    let m = rig.store.meeting(id).expect("reunión recreada");
    assert!(m.recovered);
    assert!(m.title.contains("recuperada"));
    assert!(m.duration.0 >= 1_000);
    assert_eq!(read_manifest(&rig.layout, id).unwrap().status, ManifestStatus::Stopped);
    let snap = h.snapshot().await.unwrap();
    if snap.state.name() != "ready" {
        wait_state(&mut rx, "ready").await;
    }
}

#[tokio::test]
async fn screen_frames_are_renamed_to_media_time_and_linked() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.screen.enabled = true;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();
    let id = detect_and_accept(&rig, &h, &mut rx).await;
    assert_eq!(rig.screen.requests.lock().unwrap()[0].out_dir, rig.layout.captures_dir(id));

    rig.screen.push(ScreenEvent::ShareStarted { presenter: Some("Ana".into()), at: T0.plus_ms(1_000) });
    let frame = rig.screen.write_frame(id, T0.plus_ms(1_500), CaptureReason::SlideChange);
    rig.screen.push(ScreenEvent::FrameSaved { frame: frame.clone() });
    let mut cand = rig.screen.write_frame(id, T0.plus_ms(1_600), CaptureReason::Candidate);
    cand.phash = "fedcba9876543210".into();
    rig.screen.push(ScreenEvent::FrameSaved { frame: cand });
    let ev = wait_for(&mut rx, |e| matches!(e, SupervisorEvent::CaptureSaved(_))).await;
    let SupervisorEvent::CaptureSaved(c) = ev else { unreachable!() };
    assert_eq!(c.path, "captures/000001500-slide_change.jpg");
    assert_eq!(c.at.0, 1_500);
    assert!(rig.layout.meeting_dir(id).join(&c.path).exists());
    assert!(!frame.path.exists());
    eventually(|| rig.store.captures.lock().unwrap().len() == 2).await;
    let snap = h.snapshot().await.unwrap();
    assert_eq!(snap.capture_count, 1, "las candidatas no cuentan");
    assert!(snap.screen_sharing);
    let cands: Vec<_> = rig.store.captures.lock().unwrap().iter().filter(|c| c.reason == CaptureReason::Candidate).cloned().collect();
    assert_eq!(cands[0].path, "captures/candidates/1700000001600.jpg");
    let live = std::fs::read_to_string(rig.layout.live_screen_path(id)).unwrap();
    assert!(live.contains("shareStarted"));
}

/// Real (23-nov-2026): demasiadas capturas aunque nadie compartiera pantalla.
/// Sin compartición solo valen las que pide una persona; las casi iguales a
/// la última conservada se descartan (y su fichero se borra).
#[tokio::test]
async fn automatic_captures_are_dropped_unless_someone_shares_and_the_frame_changed() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.screen.enabled = true;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();
    let id = detect_and_accept(&rig, &h, &mut rx).await;

    let idle = rig.screen.write_frame(id, T0.plus_ms(1_000), CaptureReason::SlideChange);
    rig.screen.push(ScreenEvent::FrameSaved { frame: idle.clone() });
    let manual = rig.screen.write_frame(id, T0.plus_ms(2_000), CaptureReason::Manual);
    rig.screen.push(ScreenEvent::FrameSaved { frame: manual });
    wait_for(&mut rx, |e| matches!(e, SupervisorEvent::CaptureSaved(_))).await;
    assert!(!idle.path.exists(), "la descartada se borra");
    assert_eq!(rig.store.captures.lock().unwrap().len(), 1);

    rig.screen.push(ScreenEvent::ShareStarted { presenter: None, at: T0.plus_ms(3_000) });
    let slide = rig.screen.write_frame(id, T0.plus_ms(20_000), CaptureReason::SlideChange);
    rig.screen.push(ScreenEvent::FrameSaved { frame: slide });
    let same = rig.screen.write_frame(id, T0.plus_ms(40_000), CaptureReason::SlideChange);
    rig.screen.push(ScreenEvent::FrameSaved { frame: same.clone() });
    eventually(|| rig.store.captures.lock().unwrap().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.store.captures.lock().unwrap().len(), 2, "casi duplicada");
    assert!(!same.path.exists());
}

/// Compartiendo: una diapositiva distinta pero a menos de 8 s de la anterior
/// se descarta (animaciones, transiciones); pasados los 8 s se conserva.
#[tokio::test]
async fn slide_changes_closer_than_eight_seconds_are_dropped_through_the_actor() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.screen.enabled = true;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();
    let id = detect_and_accept(&rig, &h, &mut rx).await;
    rig.screen.push(ScreenEvent::ShareStarted { presenter: None, at: T0.plus_ms(500) });

    let frame = |at: u64, hash: &str| {
        let mut f = rig.screen.write_frame(id, T0.plus_ms(at as i64), CaptureReason::SlideChange);
        f.phash = hash.into();
        f
    };
    let first = frame(10_000, "0000000000000000");
    rig.screen.push(ScreenEvent::FrameSaved { frame: first });
    eventually(|| rig.store.captures.lock().unwrap().len() == 1).await;
    let too_soon = frame(13_000, "ffffffffffffffff");
    rig.screen.push(ScreenEvent::FrameSaved { frame: too_soon.clone() });
    let later = frame(18_500, "ffffffffffffffff");
    rig.screen.push(ScreenEvent::FrameSaved { frame: later });
    eventually(|| rig.store.captures.lock().unwrap().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.store.captures.lock().unwrap().len(), 2, "la de +3 s sobra, la de +8,5 s entra");
    assert!(!too_soon.path.exists());
}

/// Real (19-nov-2026): las grabaciones manuales (sin app detectada) no
/// intentaban capturar nada. Ahora se captura la pantalla principal.
#[tokio::test]
async fn manual_recording_without_an_app_captures_the_main_display() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.screen.enabled = true;
    let h = start(&rig, settings).await;
    let id = recording_id(&h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap().state);
    let req = rig.screen.requests.lock().unwrap()[0].clone();
    assert_eq!(req.app, None);
    assert_eq!(req.out_dir, rig.layout.captures_dir(id));
}

/// Real (19-nov-2026): sin permiso al empezar, la pastilla lo decía toda la
/// reunión y no se volvía a intentar; al arreglar el permiso seguía sin
/// capturar. Ahora se reintenta y el aviso se quita al funcionar.
#[tokio::test]
async fn denied_screen_capture_is_retried_and_the_warning_clears_when_it_works() {
    let rig = Rig::new();
    *rig.screen.deny.lock().unwrap() = 2;
    let mut settings = Settings::default();
    settings.screen.enabled = true;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();
    let id = detect_and_accept(&rig, &h, &mut rx).await;
    let is_screen = |w: &HealthWarning| w.issue == HealthIssue::PermissionMissing { permission: acta_core::permissions::PermissionKind::ScreenRecording };
    wait_for(&mut rx, |e| matches!(e, SupervisorEvent::Health(w) if is_screen(w) && !w.cleared)).await;
    assert_eq!(*rig.screen.attempts.lock().unwrap(), 1);
    // Antes del plazo no se reintenta.
    rig.clock.advance_ms(2_000);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(*rig.screen.attempts.lock().unwrap(), 1);
    // Segundo intento: sigue sin permiso (el aviso no se repite ni se limpia).
    rig.clock.advance_ms(9_000);
    eventually(|| *rig.screen.attempts.lock().unwrap() == 2).await;
    // Tercero: funciona.
    rig.clock.advance_ms(11_000);
    wait_for(&mut rx, |e| matches!(e, SupervisorEvent::Health(w) if is_screen(w) && w.cleared)).await;
    assert_eq!(*rig.screen.attempts.lock().unwrap(), 3);
    assert!(h.health().await.unwrap().iter().all(|w| !is_screen(w)), "ya no sale en la pastilla");
    // Y las capturas llegan.
    rig.screen.push(ScreenEvent::ShareStarted { presenter: None, at: T0.plus_ms(22_000) });
    let frame = rig.screen.write_frame(id, T0.plus_ms(23_000), CaptureReason::SlideChange);
    rig.screen.push(ScreenEvent::FrameSaved { frame });
    wait_for(&mut rx, |e| matches!(e, SupervisorEvent::CaptureSaved(_))).await;
    // Ya no hay más reintentos.
    rig.clock.advance_ms(60_000);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(*rig.screen.attempts.lock().unwrap(), 3);
}

/// Recortes del usuario con su herramienta (al portapapeles) → capturas de
/// la reunión, como las automáticas. En pausa no se importa nada.
#[tokio::test]
async fn clipboard_snips_become_captures_of_the_meeting_except_while_paused() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let mut rx = h.subscribe();
    let id = recording_id(&h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap().state);
    assert_eq!(rig.clipboard.out_dirs.lock().unwrap()[0], rig.layout.captures_dir(id));
    let f = rig.clipboard.copy(T0.plus_ms(2_000));
    let ev = wait_for(&mut rx, |e| matches!(e, SupervisorEvent::CaptureSaved(_))).await;
    let SupervisorEvent::CaptureSaved(c) = ev else { unreachable!() };
    assert_eq!(c.reason, CaptureReason::Manual);
    assert_eq!(c.path, "captures/000002000-manual.jpg");
    assert!(rig.layout.meeting_dir(id).join(&c.path).exists());
    assert!(!f.path.exists());
    assert_eq!(h.snapshot().await.unwrap().capture_count, 1);

    h.send(SessionEvent::PauseRequested { at: T0.plus_ms(3_000) }).await.unwrap();
    let paused = rig.clipboard.copy(T0.plus_ms(3_500));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.store.captures.lock().unwrap().len(), 1, "en pausa no se importa");
    assert!(!paused.path.exists(), "y no se deja el fichero suelto");

    h.send(SessionEvent::StopRequested { reason: StopReason::User, at: T0.plus_ms(4_000) }).await.unwrap();
    eventually(|| !rig.clipboard.running()).await;
}

#[tokio::test]
async fn keyword_in_live_preview_takes_a_snapshot() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.screen.enabled = true;
    settings.transcription.live_preview = true;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();
    detect_and_accept(&rig, &h, &mut rx).await;
    rig.live.push(LivePartial { track: Track::System, text: "esto es importante".into(), at: T0.plus_ms(900), is_final: true });
    let ev = wait_for(&mut rx, |e| matches!(e, SupervisorEvent::CaptureSaved(_))).await;
    let SupervisorEvent::CaptureSaved(c) = ev else { unreachable!() };
    assert_eq!(c.reason, CaptureReason::Keyword);
    assert_eq!(c.keyword.as_deref(), Some("importante"));
    assert_eq!(rig.screen.snapshots.lock().unwrap()[0].0, CaptureReason::Keyword);
    // Y la captura manual también pasa por el sidecar.
    let manual = h.capture_manual().await.unwrap().expect("captura");
    assert_eq!(manual.reason, CaptureReason::Manual);
}

#[tokio::test]
async fn processing_never_blocks_a_new_recording() {
    let gate = Arc::new(Notify::new());
    let rig = Rig::with_pipeline(FakePipeline { gate: Some(gate.clone()), ..FakePipeline::default() });
    let h = start(&rig, Settings::default()).await;
    let first = recording_id(&h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap().state);
    let snap = h.send(SessionEvent::StopRequested { reason: StopReason::User, at: T0.plus_ms(10) }).await.unwrap();
    assert_eq!(snap.state.name(), "processing");
    eventually(|| !rig.pipeline.runs.lock().unwrap().is_empty()).await;
    let snap = h.send(SessionEvent::StartRequested { app: None, at: T0.plus_ms(20) }).await.unwrap();
    let second = recording_id(&snap.state);
    assert_ne!(first, second);
    gate.notify_one();
    // El fin del procesado de la primera no saca a la segunda de Recording.
    eventually(|| rig.store.meeting(first).unwrap().status == MeetingStatus::Ready).await;
    assert_eq!(h.snapshot().await.unwrap().state.name(), "recording");
}

#[tokio::test]
async fn audio_stream_dying_stops_and_keeps_what_was_recorded() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let mut rx = h.subscribe();
    let id = recording_id(&h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap().state);
    rig.audio.frames(Track::Mic, 0, 5, 3);
    rig.audio.end_stream();
    wait_state(&mut rx, "processing").await;
    assert_eq!(read_manifest(&rig.layout, id).unwrap().stop_reason, Some(StopReason::Error));
}

#[tokio::test]
async fn pause_and_resume_drive_audio_and_writer() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let id = recording_id(&h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap().state);
    rig.audio.frames(Track::Mic, 0, 10, 3);
    wait_elapsed(&h, 1_000).await;
    let s = h.send(SessionEvent::PauseRequested { at: T0.plus_ms(1_000) }).await.unwrap();
    assert_eq!(s.state.name(), "paused");
    assert_eq!(read_manifest(&rig.layout, id).unwrap().status, ManifestStatus::Paused);
    let s = h.send(SessionEvent::ResumeRequested { at: T0.plus_ms(60_000) }).await.unwrap();
    assert_eq!(s.state.name(), "recording");
    assert_eq!(rig.audio.calls(), vec!["start", "pause", "resume"]);
    let m = read_manifest(&rig.layout, id).unwrap();
    assert_eq!(m.spans.len(), 2);
    assert_eq!(m.spans[1].media_start.0, 1_000);
}

#[tokio::test]
async fn speaker_signals_are_logged_and_pid_changes_retarget_the_tap() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let mut rx = h.subscribe();
    let id = detect_and_accept(&rig, &h, &mut rx).await;
    rig.speakers.push(SpeakerSignal::Roster { participants: vec![Participant { name: "Ana".into(), is_self: false }], at: T0 });
    rig.speakers.push(SpeakerSignal::Active { names: vec!["Ana".into()], at: T0.plus_ms(100) });
    let path = rig.layout.live_speakers_path(id);
    eventually(|| std::fs::read_to_string(&path).map(|s| s.lines().count() == 2).unwrap_or(false)).await;

    let mut app = teams_app();
    app.audio_pids = vec![42, 43, 99];
    rig.detector.push(DetectionEvent::AudioPidsChanged { app, at: T0.plus_ms(200) });
    eventually(|| rig.audio.retargets.lock().unwrap().iter().any(|t| t.pids == vec![42, 43, 99])).await;
    let _ = h;
}

#[tokio::test]
async fn invalid_actions_are_rejected_without_changing_state() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let e = h.send(SessionEvent::PauseRequested { at: UnixMs(1) }).await.unwrap_err();
    assert!(matches!(e, acta_core::PortError::InvalidInput(_)));
    assert_eq!(h.snapshot().await.unwrap().state, SessionState::Idle);
}

#[tokio::test]
async fn shutdown_stops_the_recording_cleanly() {
    let rig = Rig::new();
    let h = start(&rig, Settings::default()).await;
    let id = recording_id(&h.send(SessionEvent::StartRequested { app: None, at: T0 }).await.unwrap().state);
    rig.audio.frames(Track::Mic, 0, 3, 3);
    h.shutdown().await.unwrap();
    assert_eq!(read_manifest(&rig.layout, id).unwrap().stop_reason, Some(StopReason::AppQuit));
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn auto_record_starts_immediately_and_stopping_within_window_discards_everything() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.detection.auto_record = true;
    settings.detection.auto_record_delay_s = 10;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();

    rig.detector.push(DetectionEvent::MeetingStarted { app: teams_app(), confidence: Confidence::High, at: T0 });
    let snap = wait_state(&mut rx, "recording").await;
    let id = recording_id(&snap.state);
    assert!(rig.store.meeting(id).is_some());
    assert!(rig.layout.meeting_dir(id).exists());
    let SessionState::Recording(rec) = &snap.state else { unreachable!() };
    assert!(matches!(
        rec.stop_prompt.as_ref().map(|p| &p.kind),
        Some(acta_core::recording::PromptKind::StopRecording { reason: acta_core::recording::StopPromptReason::AutoStarted })
    ));

    // Parar dentro de los 10 s descarta la grabación, borra la BD y la carpeta y vuelve a Idle.
    let snap = h
        .send(SessionEvent::StopRequested { reason: StopReason::User, at: T0.plus_ms(4_000) })
        .await
        .unwrap();
    assert_eq!(snap.state, SessionState::Idle);
    assert!(rig.store.meeting(id).is_none());
    assert!(!rig.layout.meeting_dir(id).exists());
    assert!(rig.pipeline.runs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn auto_record_keeps_and_processes_once_the_discard_window_passes() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.detection.auto_record = true;
    settings.detection.auto_record_delay_s = 10;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();

    rig.detector.push(DetectionEvent::MeetingStarted { app: teams_app(), confidence: Confidence::High, at: T0 });
    let snap = wait_state(&mut rx, "recording").await;
    let id = recording_id(&snap.state);

    // Pasados los 10 s el aviso de descarte se cierra solo.
    rig.clock.advance_ms(11_000);
    wait_for(&mut rx, |e| {
        matches!(e, SupervisorEvent::Snapshot(s) if matches!(&s.state, SessionState::Recording(r) if r.stop_prompt.is_none()))
    })
    .await;

    // Parar después de la ventana guarda y procesa la reunión con normalidad.
    let snap = h
        .send(SessionEvent::StopRequested { reason: StopReason::User, at: T0.plus_ms(15_000) })
        .await
        .unwrap();
    assert_eq!(snap.state.name(), "processing");
    assert!(rig.store.meeting(id).is_some());
    assert!(rig.layout.meeting_dir(id).exists());
}

#[tokio::test]
async fn auto_record_with_zero_discard_window_saves_even_if_stopped_early() {
    let rig = Rig::new();
    let mut settings = Settings::default();
    settings.detection.auto_record = true;
    settings.detection.auto_record_delay_s = 0;
    let h = start(&rig, settings).await;
    let mut rx = h.subscribe();

    rig.detector.push(DetectionEvent::MeetingStarted { app: teams_app(), confidence: Confidence::High, at: T0 });
    let snap = wait_state(&mut rx, "recording").await;
    let id = recording_id(&snap.state);
    let SessionState::Recording(rec) = &snap.state else { unreachable!() };
    assert!(rec.stop_prompt.is_none());

    let snap = h
        .send(SessionEvent::StopRequested { reason: StopReason::User, at: T0.plus_ms(2_000) })
        .await
        .unwrap();
    assert_eq!(snap.state.name(), "processing");
    assert!(rig.store.meeting(id).is_some());
}
