#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Fakes de todos los puertos que usa el supervisor.

use acta_audio::supervisor::{SupervisorEvent, SupervisorPorts};
use acta_core::detection::*;
use acta_core::ids::*;
use acta_core::layout::DataLayout;
use acta_core::library::*;
use acta_core::minutes::{ImportantMomentDetector, KeywordHit, Minutes, MinutesOutput};
use acta_core::processing::{ProcessingPipeline, ProcessingStep, ProgressSink};
use acta_core::recording::*;
use acta_core::screen::*;
use acta_core::time::{ManualClock, MediaMs, UnixMs};
use acta_core::transcription::{LivePartial, LiveTranscriber, Transcript};
use acta_core::{EventStream, PortError, PortResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, broadcast, mpsc};
use tokio_stream_shim::receiver_stream;

pub const T0: UnixMs = UnixMs(1_700_000_000_000);

/// Stream a partir de un mpsc (sin depender de tokio-stream).
mod tokio_stream_shim {
    use acta_core::EventStream;
    use tokio::sync::mpsc;
    pub fn receiver_stream<T: Send + 'static>(rx: mpsc::UnboundedReceiver<T>) -> EventStream<T> {
        Box::pin(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|x| (x, rx)) }))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}

// --------------------------------------------------------------------------
// Audio
// --------------------------------------------------------------------------
#[derive(Default)]
pub struct FakeAudio {
    pub tx: Mutex<Option<mpsc::UnboundedSender<AudioEvent>>>,
    pub requests: Mutex<Vec<CaptureRequest>>,
    pub calls: Mutex<Vec<&'static str>>,
    pub retargets: Mutex<Vec<SystemTarget>>,
    pub fail_start: Mutex<Option<PortError>>,
}

impl FakeAudio {
    pub fn push(&self, ev: AudioEvent) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(ev);
        }
    }
    pub fn frames(&self, track: Track, from: u64, n: u64, value: i16) {
        for i in from..from + n {
            self.push(AudioEvent::Frame(AudioFrame {
                track,
                seq: i,
                pts_us: i * 100_000,
                sample_rate: 16_000,
                samples: vec![value; 1600],
            }));
        }
    }
    /// Cierra el stream como si el sidecar se rindiera.
    pub fn end_stream(&self) {
        lock(&self.tx).take();
    }
    pub fn calls(&self) -> Vec<&'static str> {
        lock(&self.calls).clone()
    }
}

#[async_trait]
impl AudioSource for FakeAudio {
    async fn start(&self, request: CaptureRequest) -> PortResult<(CaptureStarted, EventStream<AudioEvent>)> {
        lock(&self.calls).push("start");
        if let Some(e) = lock(&self.fail_start).take() {
            return Err(e);
        }
        lock(&self.requests).push(request);
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.tx) = Some(tx);
        Ok((
            CaptureStarted {
                started_at: T0,
                mic_device: Some("Mic".into()),
                system_pids: vec![],
            },
            receiver_stream(rx),
        ))
    }
    async fn pause(&self) -> PortResult<()> {
        lock(&self.calls).push("pause");
        Ok(())
    }
    async fn resume(&self) -> PortResult<()> {
        lock(&self.calls).push("resume");
        Ok(())
    }
    async fn stop(&self) -> PortResult<()> {
        lock(&self.calls).push("stop");
        lock(&self.tx).take();
        Ok(())
    }
    async fn retarget(&self, system: SystemTarget) -> PortResult<()> {
        lock(&self.retargets).push(system);
        Ok(())
    }
}

// --------------------------------------------------------------------------
// Detección, nombres, pantalla, vivo
// --------------------------------------------------------------------------
#[derive(Default)]
pub struct FakeDetector {
    pub tx: Mutex<Option<mpsc::UnboundedSender<DetectionEvent>>>,
    pub configs: Mutex<Vec<DetectionConfig>>,
}
impl FakeDetector {
    pub fn push(&self, ev: DetectionEvent) {
        lock(&self.tx).as_ref().expect("detector started").send(ev).unwrap();
    }
}
#[async_trait]
impl MeetingDetector for FakeDetector {
    async fn start(&self, config: DetectionConfig) -> PortResult<EventStream<DetectionEvent>> {
        lock(&self.configs).push(config);
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.tx) = Some(tx);
        Ok(receiver_stream(rx))
    }
    async fn stop(&self) -> PortResult<()> {
        lock(&self.tx).take();
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeSpeakers {
    pub tx: Mutex<Option<mpsc::UnboundedSender<SpeakerSignal>>>,
    pub watched: Mutex<Vec<String>>,
}
impl FakeSpeakers {
    pub fn push(&self, s: SpeakerSignal) {
        lock(&self.tx).as_ref().expect("watching").send(s).unwrap();
    }
}
#[async_trait]
impl SpeakerNameSource for FakeSpeakers {
    async fn watch(&self, app: &MeetingApp) -> PortResult<EventStream<SpeakerSignal>> {
        lock(&self.watched).push(app.bundle_id.clone());
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.tx) = Some(tx);
        Ok(receiver_stream(rx))
    }
    async fn unwatch(&self) -> PortResult<()> {
        lock(&self.tx).take();
        Ok(())
    }
}

pub struct FakeScreen {
    pub tx: Mutex<Option<mpsc::UnboundedSender<ScreenEvent>>>,
    pub requests: Mutex<Vec<ScreenCaptureRequest>>,
    /// Cuántos `start` más fallan con «sin permiso» (TCC denegado).
    pub deny: Mutex<u32>,
    /// Intentos de `start`, también los fallidos.
    pub attempts: Mutex<u32>,
    pub snapshots: Mutex<Vec<(CaptureReason, UnixMs, Option<String>)>>,
    pub layout: DataLayout,
}
impl FakeScreen {
    pub fn new(layout: DataLayout) -> Self {
        Self {
            tx: Mutex::new(None),
            requests: Mutex::new(vec![]),
            deny: Mutex::new(0),
            attempts: Mutex::new(0),
            snapshots: Mutex::new(vec![]),
            layout,
        }
    }
    pub fn push(&self, ev: ScreenEvent) {
        lock(&self.tx).as_ref().expect("screen started").send(ev).unwrap();
    }
    /// Escribe un jpg falso como lo haría el sidecar (`<unix_ms>-<reason>.jpg`).
    pub fn write_frame(&self, meeting: MeetingId, at: UnixMs, reason: CaptureReason) -> CapturedFrame {
        let (dir, name) = match reason {
            CaptureReason::Candidate => (self.layout.capture_candidates_dir(meeting), format!("{}.jpg", at.0)),
            r => (self.layout.captures_dir(meeting), format!("{}-{}.jpg", at.0, acta_audio::supervisor::files::reason_wire(r))),
        };
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"jpg").unwrap();
        CapturedFrame { path, at, reason, phash: "0123456789abcdef".into(), width: 640, height: 360, keyword: None }
    }
}
#[async_trait]
impl ScreenCapturer for FakeScreen {
    async fn start(&self, request: ScreenCaptureRequest) -> PortResult<EventStream<ScreenEvent>> {
        *lock(&self.attempts) += 1;
        {
            let mut deny = lock(&self.deny);
            if *deny > 0 {
                *deny -= 1;
                return Err(PortError::PermissionDenied("screen recording permission not granted".into()));
            }
        }
        lock(&self.requests).push(request);
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.tx) = Some(tx);
        Ok(receiver_stream(rx))
    }
    async fn snapshot(&self, reason: CaptureReason, at: UnixMs, keyword: Option<String>) -> PortResult<Option<CapturedFrame>> {
        lock(&self.snapshots).push((reason, at, keyword.clone()));
        let meeting = lock(&self.requests).last().map(|r| r.meeting_id).expect("started");
        let mut f = self.write_frame(meeting, at, reason);
        f.keyword = keyword;
        Ok(Some(f))
    }
    async fn stop(&self) -> PortResult<()> {
        lock(&self.tx).take();
        Ok(())
    }
}

/// Portapapeles falso: `copy` hace lo que el sidecar al ver una imagen nueva.
#[derive(Default)]
pub struct FakeClipboard {
    pub tx: Mutex<Option<mpsc::UnboundedSender<CapturedFrame>>>,
    pub out_dirs: Mutex<Vec<std::path::PathBuf>>,
}
impl FakeClipboard {
    pub fn copy(&self, at: UnixMs) -> CapturedFrame {
        let dir = lock(&self.out_dirs).last().cloned().expect("clipboard started");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}-manual.jpg", at.0));
        std::fs::write(&path, b"jpg").unwrap();
        let f = CapturedFrame { path, at, reason: CaptureReason::Manual, phash: "0f0f0f0f0f0f0f0f".into(), width: 800, height: 600, keyword: None };
        lock(&self.tx).as_ref().expect("clipboard started").send(f.clone()).unwrap();
        f
    }
    pub fn running(&self) -> bool {
        lock(&self.tx).is_some()
    }
}
#[async_trait]
impl ClipboardImporter for FakeClipboard {
    async fn start(&self, out_dir: std::path::PathBuf, _max_width: u32) -> PortResult<EventStream<CapturedFrame>> {
        lock(&self.out_dirs).push(out_dir);
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.tx) = Some(tx);
        Ok(receiver_stream(rx))
    }
    async fn stop(&self) -> PortResult<()> {
        lock(&self.tx).take();
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeLive {
    pub tx: Mutex<Option<mpsc::UnboundedSender<LivePartial>>>,
}
impl FakeLive {
    pub fn push(&self, p: LivePartial) {
        lock(&self.tx).as_ref().expect("live started").send(p).unwrap();
    }
}
#[async_trait]
impl LiveTranscriber for FakeLive {
    async fn start(&self, _tracks: &[Track], _language: Option<String>) -> PortResult<EventStream<LivePartial>> {
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.tx) = Some(tx);
        Ok(receiver_stream(rx))
    }
    async fn stop(&self) -> PortResult<()> {
        lock(&self.tx).take();
        Ok(())
    }
}

/// Detecta "importante" (el real vive en acta-llm).
pub struct FakeMoments;
impl ImportantMomentDetector for FakeMoments {
    fn detect(&self, text: &str, track: Track, at: UnixMs) -> Option<KeywordHit> {
        text.contains("importante").then(|| KeywordHit { keyword: "importante".into(), track, at })
    }
}

// --------------------------------------------------------------------------
// Store en memoria (solo lo que usa el supervisor)
// --------------------------------------------------------------------------
#[derive(Default)]
pub struct FakeStore {
    pub meetings: Mutex<HashMap<MeetingId, Meeting>>,
    pub captures: Mutex<Vec<Capture>>,
}
impl FakeStore {
    pub fn meeting(&self, id: MeetingId) -> Option<Meeting> {
        lock(&self.meetings).get(&id).cloned()
    }
}
fn nope<T>(what: &str) -> PortResult<T> {
    Err(PortError::NotImplemented(what.into()))
}
#[async_trait]
impl MeetingStore for FakeStore {
    async fn create_meeting(&self, m: NewMeeting) -> PortResult<Meeting> {
        let meeting = Meeting {
            id: m.id,
            title: m.title,
            platform: m.platform,
            app_bundle_id: m.app_bundle_id,
            started_at: m.started_at,
            ended_at: None,
            duration: MediaMs(0),
            status: MeetingStatus::Recording,
            recovered: false,
            error: None,
            language: m.language,
            created_at: m.started_at,
            updated_at: m.started_at,
            favourite: false,
            error_provider: None,
            error_model: None,
        };
        lock(&self.meetings).insert(m.id, meeting.clone());
        Ok(meeting)
    }
    async fn patch_meeting(&self, id: MeetingId, p: MeetingPatch) -> PortResult<Meeting> {
        let mut all = lock(&self.meetings);
        let m = all.get_mut(&id).ok_or_else(|| PortError::NotFound(id.to_string()))?;
        if let Some(t) = p.title { m.title = t; }
        if let Some(e) = p.ended_at { m.ended_at = Some(e); }
        if let Some(d) = p.duration { m.duration = d; }
        if let Some(s) = p.status { m.status = s; }
        if let Some(r) = p.recovered { m.recovered = r; }
        if let Some(e) = p.error { m.error = e; }
        Ok(m.clone())
    }
    async fn get_meeting(&self, id: MeetingId) -> PortResult<Option<Meeting>> {
        Ok(self.meeting(id))
    }
    async fn get_meeting_detail(&self, _id: MeetingId) -> PortResult<Option<MeetingDetail>> { nope("detail") }
    async fn list_meetings(&self, _q: MeetingQuery) -> PortResult<Page<MeetingSummary>> {
        let items = lock(&self.meetings)
            .values()
            .map(|m| MeetingSummary {
                id: m.id,
                title: m.title.clone(),
                platform: m.platform,
                started_at: m.started_at,
                duration: m.duration,
                status: m.status,
                speaker_names: vec![],
                my_open_tasks: 0,
                group_ids: vec![],
                capture_count: 0,
                snippet: None,
                favourite: false,
            })
            .collect();
        Ok(Page { items, next_cursor: None })
    }
    async fn delete_meeting(&self, id: MeetingId) -> PortResult<()> {
        self.meetings.lock().unwrap().remove(&id);
        Ok(())
    }
    async fn set_meeting_favourite(&self, _id: MeetingId, _f: bool) -> PortResult<Meeting> { nope("favourite") }
    async fn touch_meeting(&self, _id: MeetingId, _t: MeetingTouch, _at: acta_core::time::UnixMs) -> PortResult<()> { Ok(()) }
    async fn set_cleanup_keep(&self, _id: MeetingId, _k: bool) -> PortResult<()> { nope("keep") }
    async fn cleanup_facts(&self) -> PortResult<Vec<MeetingCleanupFacts>> { nope("cleanup_facts") }
    async fn save_transcript(&self, _t: Transcript) -> PortResult<()> { nope("save_transcript") }
    async fn get_transcript(&self, _id: MeetingId) -> PortResult<Option<Transcript>> { nope("get_transcript") }
    async fn rename_speaker(&self, _m: MeetingId, _s: SpeakerId, _n: String, _c: bool) -> PortResult<()> { nope("rename") }
    async fn link_speaker_voiceprint(&self, _s: SpeakerId, _v: Option<VoiceprintId>) -> PortResult<()> { nope("link") }
    async fn save_minutes(&self, _o: MinutesOutput) -> PortResult<()> { nope("save_minutes") }
    async fn get_minutes(&self, _id: MeetingId) -> PortResult<Option<Minutes>> { nope("get_minutes") }
    async fn set_action_item_done(&self, _id: ActionItemId, _d: bool) -> PortResult<()> { nope("done") }
    async fn add_capture(&self, c: Capture) -> PortResult<()> {
        lock(&self.captures).push(c);
        Ok(())
    }
    async fn update_capture(&self, _c: Capture) -> PortResult<()> { nope("update_capture") }
    async fn list_captures(&self, id: MeetingId) -> PortResult<Vec<Capture>> {
        Ok(lock(&self.captures).iter().filter(|c| c.meeting_id == id).cloned().collect())
    }
    async fn get_capture(&self, id: CaptureId) -> PortResult<Option<Capture>> {
        Ok(lock(&self.captures).iter().find(|c| c.id == id).cloned())
    }
    async fn delete_capture(&self, _id: CaptureId) -> PortResult<()> { nope("delete_capture") }
    async fn save_screen_shares(&self, _m: MeetingId, _s: Vec<ScreenShare>) -> PortResult<()> { nope("shares") }
}

// --------------------------------------------------------------------------
// Pipeline
// --------------------------------------------------------------------------
#[derive(Default)]
pub struct FakePipeline {
    pub runs: Mutex<Vec<(MeetingId, Vec<ProcessingStep>)>>,
    /// Si está, cada ejecución espera a `release.notify_one()`.
    pub gate: Option<Arc<Notify>>,
    pub fail: Mutex<Option<PortError>>,
}
#[async_trait]
impl ProcessingPipeline for FakePipeline {
    async fn run(&self, id: MeetingId, steps: &[ProcessingStep], progress: &dyn ProgressSink) -> PortResult<()> {
        lock(&self.runs).push((id, steps.to_vec()));
        progress.progress(id, ProcessingStep::Transcribing, 0.5);
        if let Some(g) = &self.gate {
            g.notified().await;
        }
        match lock(&self.fail).take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

// --------------------------------------------------------------------------
// Montaje
// --------------------------------------------------------------------------
pub struct Rig {
    pub dir: tempfile::TempDir,
    pub layout: DataLayout,
    pub clock: ManualClock,
    pub audio: Arc<FakeAudio>,
    pub detector: Arc<FakeDetector>,
    pub speakers: Arc<FakeSpeakers>,
    pub screen: Arc<FakeScreen>,
    pub clipboard: Arc<FakeClipboard>,
    pub live: Arc<FakeLive>,
    pub store: Arc<FakeStore>,
    pub pipeline: Arc<FakePipeline>,
}

impl Rig {
    pub fn new() -> Self {
        Self::with_pipeline(FakePipeline::default())
    }
    pub fn with_pipeline(pipeline: FakePipeline) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        Self {
            screen: Arc::new(FakeScreen::new(layout.clone())),
            layout,
            dir,
            clock: ManualClock::new(T0),
            audio: Arc::default(),
            detector: Arc::default(),
            speakers: Arc::default(),
            clipboard: Arc::default(),
            live: Arc::default(),
            store: Arc::default(),
            pipeline: Arc::new(pipeline),
        }
    }
    pub fn ports(&self) -> SupervisorPorts {
        SupervisorPorts {
            layout: self.layout.clone(),
            clock: Arc::new(self.clock.clone()),
            audio: self.audio.clone(),
            detector: self.detector.clone(),
            speakers: self.speakers.clone(),
            screen: Some(self.screen.clone()),
            clipboard: Some(self.clipboard.clone()),
            live: Some(self.live.clone()),
            live_asr: None,
            live_sessions: Arc::default(),
            moments: Arc::new(FakeMoments),
            store: self.store.clone(),
            pipeline: self.pipeline.clone(),
            disk_free: Arc::new(|_| Some(500 * 1024 * 1024 * 1024)),
        }
    }
}

pub fn teams_app() -> MeetingApp {
    MeetingApp {
        platform: Platform::Teams,
        bundle_id: "com.microsoft.teams2".into(),
        pid: 42,
        audio_pids: vec![42, 43],
        name: "Microsoft Teams".into(),
        title: Some("Daily | Microsoft Teams".into()),
        url: None,
    }
}

/// Espera (≤ 5 s) a un evento que cumpla `pred`.
pub async fn wait_for<F>(rx: &mut broadcast::Receiver<SupervisorEvent>, mut pred: F) -> SupervisorEvent
where
    F: FnMut(&SupervisorEvent) -> bool,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(ev) if pred(&ev) => return ev,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(e) => panic!("canal cerrado: {e}"),
            }
        }
    })
    .await
    .expect("evento esperado no llegó")
}

/// Espera a un snapshot con el estado `name`.
pub async fn wait_state(rx: &mut broadcast::Receiver<SupervisorEvent>, name: &str) -> acta_core::api::SessionSnapshot {
    match wait_for(rx, |e| matches!(e, SupervisorEvent::Snapshot(s) if s.state.name() == name)).await {
        SupervisorEvent::Snapshot(s) => s,
        _ => unreachable!(),
    }
}

/// Reintenta `f` hasta 5 s.
pub async fn eventually<F: FnMut() -> bool>(mut f: F) {
    for _ in 0..250 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("la condición no se cumplió a tiempo");
}
