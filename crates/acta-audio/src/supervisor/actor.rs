//! Bucle del supervisor: aplica eventos a la máquina de estados y ejecuta
//! sus efectos EN ORDEN. Los efectos que generan eventos de vuelta
//! (`CaptureFinalized`, `RecoveryFinished`, `Fatal`...) los encolan en
//! `pending`, que se vacía antes de atender el siguiente mensaje.

use super::capture::{CaptureCore, SharedCore, lock, spawn_pump};
use super::files::{append_jsonl, place_frame};
use super::queue::{Queue, spawn_queue};
use super::{Msg, Request, SupervisorConfig, SupervisorEvent, SupervisorPorts};
use crate::processing::manifest_duration;
use crate::recovery::{recover, scan_incomplete};
use crate::watchdog::Watchdog;
use crate::writer::{SegmentWriter, SegmentWriterConfig, read_manifest};
use acta_core::EventStream;
use acta_core::api::{MeetingUpdated, SessionSnapshot};
use acta_core::detection::{DetectionConfig, DetectionEvent, MeetingApp, Platform};
use acta_core::ids::MeetingId;
use acta_core::library::{MeetingPatch, MeetingQuery, MeetingStatus, NewMeeting};
use acta_core::permissions::PermissionKind;
use acta_core::processing::ProcessingStep;
use acta_core::recording::{
    AudioFormat, AutoStopPolicy, CaptureRequest, Effect, HealthIssue, HealthWarning, IdSource, ManifestStatus,
    MicTarget, PromptAnswer, PromptKind, RandomIds, SessionEvent, SessionState, StopPromptReason, StopReason,
    SystemTarget, Track,
};
use acta_core::screen::{Capture, CaptureReason, CapturedFrame, ScreenCaptureRequest, ScreenEvent};
use acta_core::settings::Settings;
use acta_core::time::{MediaMs, UnixMs};
use acta_core::{PortError, PortResult};
use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

/// Título provisional (el LLM sugiere uno mejor si el usuario no lo cambia).
pub(crate) fn default_title(app: Option<&MeetingApp>) -> String {
    if let Some(t) = app.and_then(|a| a.title.as_deref()) {
        let t = t.split(" | ").next().unwrap_or(t).trim();
        if !t.is_empty() {
            return t.to_owned();
        }
    }
    match app.map(|a| a.platform) {
        Some(Platform::Teams) => "Reunión de Teams".into(),
        Some(Platform::Meet) => "Reunión de Meet".into(),
        Some(Platform::Zoom) => "Reunión de Zoom".into(),
        Some(Platform::Slack) => "Huddle de Slack".into(),
        Some(Platform::Other) | None => "Grabación".into(),
    }
}

fn warning_key(issue: &HealthIssue) -> String {
    match issue {
        HealthIssue::TrackStalled { track } => format!("stalled:{}", track.as_str()),
        HealthIssue::TrackAllZeros { track } => format!("zeros:{}", track.as_str()),
        HealthIssue::SegmentsStalled { track } => format!("segments:{}", track.as_str()),
        HealthIssue::DiskLow { .. } => "disk".into(),
        HealthIssue::Overrun { track, .. } => format!("overrun:{}", track.as_str()),
        HealthIssue::SidecarRestarted { .. } => "sidecar".into(),
        HealthIssue::WriteFailed { track, .. } => format!("write:{}", track.as_str()),
        HealthIssue::PermissionMissing { permission } => format!("perm:{permission:?}"),
    }
}

/// Cada cuánto se reintenta la captura de pantalla si no arrancó (permiso
/// concedido a mitad de reunión, ventana que aún no existía...).
pub(crate) const SCREEN_RETRY_MS: i64 = 10_000;

fn forward<T: Send + 'static>(
    mut stream: EventStream<T>,
    tx: UnboundedSender<Msg>,
    wrap: impl Fn(T) -> Msg + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            if tx.send(wrap(item)).is_err() {
                break;
            }
        }
    })
}

struct Recording {
    meeting_id: MeetingId,
    app: Option<MeetingApp>,
    gen_id: u64,
    title: String,
    core: SharedCore,
    autostop: AutoStopPolicy,
    pump: Option<JoinHandle<()>>,
    tasks: Vec<JoinHandle<()>>,
    speakers_on: bool,
    screen_on: bool,
    /// Petición de captura de pantalla pendiente de arrancar (falló al
    /// empezar) y cuándo reintentarla.
    screen_retry: Option<(ScreenCaptureRequest, UnixMs)>,
    clipboard_on: bool,
    live_on: bool,
    screen_sharing: bool,
    capture_count: u32,
}

pub(crate) struct Actor {
    ports: SupervisorPorts,
    settings: Settings,
    config: SupervisorConfig,
    state: SessionState,
    ids: RandomIds,
    tx: UnboundedSender<Msg>,
    events: broadcast::Sender<SupervisorEvent>,
    queue: Queue,
    rec: Option<Recording>,
    next_gen: u64,
    warnings: Vec<HealthWarning>,
    detector_task: Option<JoinHandle<()>>,
    pending: VecDeque<SessionEvent>,
}

impl Actor {
    pub fn new(
        ports: SupervisorPorts,
        settings: Settings,
        config: SupervisorConfig,
        tx: UnboundedSender<Msg>,
        events: broadcast::Sender<SupervisorEvent>,
    ) -> Self {
        let queue = spawn_queue(ports.store.clone(), ports.pipeline.clone(), events.clone(), tx.clone());
        Self {
            ports,
            settings,
            config,
            state: SessionState::Idle,
            ids: RandomIds,
            tx,
            events,
            queue,
            rec: None,
            next_gen: 0,
            warnings: Vec::new(),
            detector_task: None,
            pending: VecDeque::new(),
        }
    }

    fn now(&self) -> UnixMs {
        self.ports.clock.now()
    }

    fn emit(&self, e: SupervisorEvent) {
        let _ = self.events.send(e);
    }

    fn snapshot(&self) -> SessionSnapshot {
        let (elapsed_ms, screen_sharing, capture_count) = match &self.rec {
            Some(r) => (lock(&r.core).recorded_ms(), r.screen_sharing, r.capture_count),
            None => (0, false, 0),
        };
        SessionSnapshot {
            state: self.state.clone(),
            elapsed_ms,
            warnings: self.warnings.clone(),
            screen_sharing,
            capture_count,
            at: self.now(),
        }
    }

    fn emit_snapshot(&self) {
        self.emit(SupervisorEvent::Snapshot(self.snapshot()));
    }

    // ------------------------------------------------------------------
    // Arranque
    // ------------------------------------------------------------------

    /// Recupera sesiones que quedaron a medias (crash), reencola reuniones
    /// sin terminar de procesar y arranca la detección.
    pub async fn startup(&mut self) {
        let layout = self.ports.layout.clone();
        let incomplete = tokio::task::spawn_blocking(move || scan_incomplete(&layout))
            .await
            .map_err(|e| PortError::Internal(e.to_string()))
            .and_then(|r| r)
            .unwrap_or_else(|e| {
                tracing::error!("no se pudieron buscar sesiones incompletas: {e}");
                Vec::new()
            });
        for (i, id) in incomplete.iter().enumerate() {
            if i == 0 {
                let _ = self.apply(SessionEvent::RecoveryRequired { meeting_id: *id }).await;
            } else if self.recover_meeting(*id).await.is_ok() {
                self.queue.enqueue(*id, ProcessingStep::ORDER.to_vec());
            }
        }
        self.requeue_unfinished(&incomplete).await;
        self.start_detector().await;
    }

    /// Reuniones en `recording`/`processing` cuyo audio ya está cerrado: la
    /// app murió procesando. Se vuelven a encolar.
    async fn requeue_unfinished(&mut self, skip: &[MeetingId]) {
        let query = MeetingQuery {
            limit: Some(200),
            ..MeetingQuery::default()
        };
        let Ok(page) = self.ports.store.list_meetings(query).await else {
            return;
        };
        for m in page.items {
            if skip.contains(&m.id) || !matches!(m.status, MeetingStatus::Recording | MeetingStatus::Processing) {
                continue;
            }
            match read_manifest(&self.ports.layout, m.id) {
                Ok(man) if matches!(man.status, ManifestStatus::Stopped | ManifestStatus::Finalized) => {
                    self.queue.enqueue(m.id, ProcessingStep::ORDER.to_vec());
                }
                Ok(_) => {}
                Err(_) => {
                    let patch = MeetingPatch {
                        status: Some(MeetingStatus::Failed),
                        error: Some(Some("No se encontró el audio de esta reunión.".into())),
                        ..MeetingPatch::default()
                    };
                    let _ = self.ports.store.patch_meeting(m.id, patch).await;
                }
            }
        }
    }

    async fn start_detector(&mut self) {
        if let Some(t) = self.detector_task.take() {
            t.abort();
        }
        let d = &self.settings.detection;
        if !d.enabled {
            let _ = self.ports.detector.stop().await;
            return;
        }
        let config = DetectionConfig {
            rules: d
                .rules
                .iter()
                .filter(|r| !d.denylist.contains(&r.bundle_id))
                .cloned()
                .collect(),
            meet_url_patterns: d.meet_url_patterns.clone(),
            poll_ms: d.poll_ms,
            debounce_ms: d.debounce_ms,
        };
        match self.ports.detector.start(config).await {
            Ok(stream) => self.detector_task = Some(forward(stream, self.tx.clone(), Msg::Detection)),
            Err(e) => tracing::warn!("no se pudo arrancar la detección de reuniones: {e}"),
        }
    }

    // ------------------------------------------------------------------
    // Bucle
    // ------------------------------------------------------------------

    pub async fn run(mut self, mut rx: UnboundedReceiver<Msg>) {
        let mut tick = tokio::time::interval(self.config.tick);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    if !self.handle(msg).await {
                        break;
                    }
                }
                _ = tick.tick() => self.on_tick().await,
            }
        }
        if let Some(t) = self.detector_task.take() {
            t.abort();
        }
    }

    fn current_gen(&self, gen_id: u64) -> bool {
        self.rec.as_ref().is_some_and(|r| r.gen_id == gen_id)
    }

    /// `false` = salir del bucle.
    async fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Req(req) => return self.on_request(req).await,
            Msg::Levels { gen_id, levels } => {
                if !self.current_gen(gen_id) {
                    return true;
                }
                self.emit(SupervisorEvent::Levels(levels));
                let ev = self.rec.as_mut().and_then(|r| r.autostop.on_levels(&levels));
                if let Some(ev) = ev {
                    let _ = self.apply(ev).await;
                }
            }
            Msg::AudioEnded { gen_id, error } => {
                if self.current_gen(gen_id)
                    && matches!(self.state, SessionState::Recording(_) | SessionState::Paused { .. })
                {
                    let error = error.unwrap_or_else(|| "la captura de audio se detuvo inesperadamente".into());
                    let at = self.now();
                    let _ = self.apply(SessionEvent::Fatal { error, at }).await;
                }
            }
            Msg::Detection(ev) => self.on_detection(ev).await,
            Msg::Speaker { gen_id, signal } => {
                if let Some(r) = self.rec.as_ref().filter(|r| r.gen_id == gen_id) {
                    let path = self.ports.layout.live_speakers_path(r.meeting_id);
                    if let Err(e) = append_jsonl(&path, &signal) {
                        tracing::warn!("no se pudo guardar la señal de hablante: {e}");
                    }
                }
            }
            Msg::Screen { gen_id, event } => {
                if self.current_gen(gen_id) {
                    self.on_screen(event).await;
                }
            }
            Msg::Clipboard { gen_id, frame } => {
                if !self.current_gen(gen_id) {
                    return true;
                }
                if matches!(self.state, SessionState::Paused { .. }) {
                    // En pausa el usuario puede estar con algo privado.
                    let _ = std::fs::remove_file(&frame.path);
                } else {
                    tracing::info!("recorte del portapapeles importado como captura");
                    self.save_frame(frame).await;
                }
            }
            Msg::Live { gen_id, partial } => {
                if !self.current_gen(gen_id) {
                    return true;
                }
                let hit = self.ports.moments.detect(&partial.text, partial.track, partial.at);
                self.emit(SupervisorEvent::LivePartial(partial));
                let screen_on = self.rec.as_ref().is_some_and(|r| r.screen_on);
                if let (Some(hit), true) = (hit, screen_on) {
                    tracing::info!(keyword = %hit.keyword, "momento importante: captura");
                    self.snapshot_frame(CaptureReason::Keyword, hit.at, Some(hit.keyword)).await;
                }
            }
            Msg::LiveTranscript { gen_id, transcript } => {
                if self.current_gen(gen_id) {
                    self.emit(SupervisorEvent::LiveTranscript(transcript));
                }
            }
            Msg::Processing(ev) => {
                let _ = self.apply(ev).await;
            }
        }
        true
    }

    async fn on_request(&mut self, req: Request) -> bool {
        match req {
            Request::Event(ev, reply) => {
                let r = self.apply(ev).await.map(|()| self.snapshot());
                let _ = reply.send(r);
            }
            Request::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            Request::Health(reply) => {
                let _ = reply.send(self.warnings.clone());
            }
            Request::UpdateSettings(s) => {
                let detection_changed = s.detection != self.settings.detection;
                self.settings = *s;
                if detection_changed {
                    self.start_detector().await;
                }
            }
            Request::Reprocess(id, steps) => self.queue.enqueue(id, steps),
            Request::CaptureManual(reply) => {
                let r = if self.rec.as_ref().is_some_and(|r| r.screen_on) {
                    let at = self.now();
                    Ok(self.snapshot_frame(CaptureReason::Manual, at, None).await)
                } else {
                    Ok(None)
                };
                let _ = reply.send(r);
            }
            Request::Shutdown(reply) => {
                if matches!(self.state, SessionState::Recording(_) | SessionState::Paused { .. }) {
                    let at = self.now();
                    let _ = self
                        .apply(SessionEvent::StopRequested {
                            reason: StopReason::AppQuit,
                            at,
                        })
                        .await;
                }
                if let Some(t) = self.detector_task.take() {
                    t.abort();
                }
                let _ = self.ports.detector.stop().await;
                let _ = reply.send(());
                return false;
            }
        }
        true
    }

    async fn on_tick(&mut self) {
        let now = self.now();
        if let SessionState::Detected { prompt, .. } = &self.state {
            let timeout_ms = i64::from(self.settings.detection.start_prompt_timeout_s) * 1000;
            if now.since(prompt.created_at) >= timeout_ms {
                let prompt_id = prompt.id;
                let _ = self.apply(SessionEvent::PromptExpired { prompt_id, at: now }).await;
            }
        }
        if let SessionState::Recording(rec) | SessionState::Paused { recording: rec, .. } = &self.state
            && let Some(prompt) = &rec.stop_prompt
            && matches!(prompt.kind, PromptKind::StopRecording { reason: StopPromptReason::AutoStarted })
        {
            let window_ms = i64::from(self.settings.detection.auto_record_delay_s) * 1000;
            if now.since(prompt.created_at) >= window_ms {
                let prompt_id = prompt.id;
                let _ = self
                    .apply(SessionEvent::PromptAnswered {
                        prompt_id,
                        answer: PromptAnswer::Decline,
                        at: now,
                    })
                    .await;
            }
        }
        self.retry_screen(now).await;
        let free = (self.ports.disk_free)(self.ports.layout.root());
        let Some(rec) = self.rec.as_mut() else { return };
        let auto = rec.autostop.on_tick(now);
        let warnings = lock(&rec.core).watchdog.tick(now, free);
        self.handle_warnings(warnings);
        if let Some(ev) = auto {
            let _ = self.apply(ev).await;
        }
        if self.rec.is_some() {
            self.emit_snapshot();
        }
    }

    fn handle_warnings(&mut self, warnings: Vec<HealthWarning>) {
        for w in warnings {
            let key = warning_key(&w.issue);
            self.warnings.retain(|x| warning_key(&x.issue) != key);
            if !w.cleared {
                self.warnings.push(w.clone());
            }
            self.emit(SupervisorEvent::Health(w));
        }
    }

    fn clear_warnings(&mut self) {
        for mut w in std::mem::take(&mut self.warnings) {
            w.cleared = true;
            self.emit(SupervisorEvent::Health(w));
        }
    }

    // ------------------------------------------------------------------
    // Detección y pantalla
    // ------------------------------------------------------------------

    async fn on_detection(&mut self, ev: DetectionEvent) {
        match ev {
            DetectionEvent::MeetingStarted { app, at, .. } => {
                if self.settings.detection.denylist.contains(&app.bundle_id) {
                    return;
                }
                if self.settings.detection.auto_record {
                    let open_discard_prompt = self.settings.detection.auto_record_delay_s > 0;
                    let _ = self.apply(SessionEvent::AutoRecordStarted { app, open_discard_prompt, at }).await;
                } else {
                    let _ = self.apply(SessionEvent::MeetingDetected { app, at }).await;
                }
            }
            DetectionEvent::MeetingEnded { app, at } => {
                let relevant = match &self.state {
                    SessionState::Detected { app: a, .. } => a.bundle_id == app.bundle_id,
                    SessionState::Recording(r) | SessionState::Paused { recording: r, .. } => {
                        r.app.as_ref().is_some_and(|a| a.bundle_id == app.bundle_id)
                    }
                    _ => false,
                };
                if relevant {
                    let _ = self.apply(SessionEvent::MeetingEnded { at }).await;
                }
            }
            DetectionEvent::AudioPidsChanged { app, .. } => {
                let Some(rec) = self.rec.as_mut() else { return };
                if rec.app.as_ref().is_none_or(|a| a.bundle_id != app.bundle_id) {
                    return;
                }
                let target = system_target(Some(&app));
                rec.app = Some(app);
                if let Err(e) = self.ports.audio.retarget(target).await {
                    tracing::warn!("no se pudo reasignar el process tap: {e}");
                }
            }
        }
    }

    async fn on_screen(&mut self, event: ScreenEvent) {
        match &event {
            ScreenEvent::ShareStarted { .. } | ScreenEvent::ShareEnded { .. } => {
                let Some(rec) = self.rec.as_mut() else { return };
                rec.screen_sharing = matches!(event, ScreenEvent::ShareStarted { .. });
                let path = self.ports.layout.live_screen_path(rec.meeting_id);
                if let Err(e) = append_jsonl(&path, &event) {
                    tracing::warn!("no se pudo guardar el estado de pantalla compartida: {e}");
                }
                self.emit_snapshot();
            }
            ScreenEvent::FrameSaved { frame } => {
                self.save_frame(frame.clone()).await;
            }
        }
    }

    async fn snapshot_frame(&mut self, reason: CaptureReason, at: UnixMs, keyword: Option<String>) -> Option<Capture> {
        let screen = self.ports.screen.clone()?;
        match screen.snapshot(reason, at, keyword).await {
            Ok(Some(frame)) => self.save_frame(frame).await,
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("no se pudo hacer la captura: {e}");
                None
            }
        }
    }

    /// Renombra la captura a tiempo media, la guarda y la anuncia.
    async fn save_frame(&mut self, frame: CapturedFrame) -> Option<Capture> {
        let rec = self.rec.as_mut()?;
        let at = lock(&rec.core).wall_to_media(frame.at);
        let path = match place_frame(&self.ports.layout, rec.meeting_id, &frame, at) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("no se pudo colocar la captura {}: {e}", frame.path.display());
                return None;
            }
        };
        let capture = Capture {
            id: acta_core::ids::CaptureId::new(),
            meeting_id: rec.meeting_id,
            at,
            at_wall: frame.at,
            path,
            reason: frame.reason,
            phash: frame.phash,
            width: frame.width,
            height: frame.height,
            keyword: frame.keyword,
            topic_path: None,
            action_item_ids: vec![],
        };
        if let Err(e) = self.ports.store.add_capture(capture.clone()).await {
            tracing::warn!("no se pudo guardar la captura: {e}");
            return None;
        }
        if capture.reason != CaptureReason::Candidate {
            rec.capture_count += 1;
            self.emit(SupervisorEvent::CaptureSaved(capture.clone()));
        }
        Some(capture)
    }

    // ------------------------------------------------------------------
    // Máquina de estados + efectos
    // ------------------------------------------------------------------

    /// Aplica `event` y todos los que generen sus efectos. Devuelve el
    /// resultado del PRIMERO (lo que ve el llamador).
    async fn apply(&mut self, event: SessionEvent) -> PortResult<()> {
        let first = self.step(event).await;
        while let Some(next) = self.pending.pop_front() {
            if let Err(e) = self.step(next).await {
                tracing::debug!("evento interno ignorado: {e}");
            }
        }
        first
    }

    async fn step(&mut self, event: SessionEvent) -> PortResult<()> {
        let t = match self.state.transition(event, &mut self.ids as &mut dyn IdSource) {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!("{e}");
                return Err(PortError::InvalidInput(format!("acción no válida en este momento ({e})")));
            }
        };
        let changed = t.next != self.state || !t.effects.is_empty();
        self.state = t.next;
        for effect in t.effects {
            self.run_effect(effect).await;
        }
        if changed {
            self.emit_snapshot();
        }
        Ok(())
    }

    async fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::ShowPrompt { prompt } => {
                if let (PromptKind::StopRecording { reason }, Some(rec)) = (&prompt.kind, self.rec.as_mut())
                    && *reason != StopPromptReason::AutoStarted
                {
                    rec.autostop.on_stop_prompt_opened(prompt.created_at);
                }
            }
            Effect::ClosePrompt { .. } => {
                if matches!(self.state, SessionState::Recording(_) | SessionState::Paused { .. })
                    && let Some(rec) = self.rec.as_mut()
                {
                    rec.autostop.on_stop_prompt_closed();
                }
            }
            Effect::StartCapture { meeting_id, app, at } => self.start_capture(meeting_id, app, at).await,
            Effect::PauseCapture { at, .. } => {
                if let Err(e) = self.ports.audio.pause().await {
                    tracing::warn!("pausa del audio: {e}");
                }
                if let Some(rec) = self.rec.as_mut() {
                    let mut core = lock(&rec.core);
                    if let Some(w) = core.writer.as_mut()
                        && let Err(e) = w.pause(at)
                    {
                        tracing::error!("pausa del writer: {e}");
                    }
                    core.watchdog.on_paused(at);
                    drop(core);
                    rec.autostop.on_paused(at);
                }
            }
            Effect::ResumeCapture { at, .. } => {
                if let Some(rec) = self.rec.as_mut() {
                    let mut core = lock(&rec.core);
                    if let Some(w) = core.writer.as_mut()
                        && let Err(e) = w.resume(at)
                    {
                        tracing::error!("reanudación del writer: {e}");
                    }
                    core.watchdog.on_resumed(at);
                    drop(core);
                    rec.autostop.on_resumed(at);
                }
                if let Err(e) = self.ports.audio.resume().await {
                    tracing::warn!("reanudación del audio: {e}");
                }
            }
            Effect::StopCapture { meeting_id, reason, at } => self.stop_capture(meeting_id, reason, at).await,
            Effect::StartProcessing { meeting_id } => {
                self.queue.enqueue(meeting_id, ProcessingStep::ORDER.to_vec());
            }
            Effect::RunRecovery { meeting_id } => match self.recover_meeting(meeting_id).await {
                Ok(()) => self.pending.push_back(SessionEvent::RecoveryFinished { meeting_id }),
                Err(e) => {
                    let error = format!("No se pudo recuperar la grabación: {e}");
                    let patch = MeetingPatch {
                        status: Some(MeetingStatus::Failed),
                        error: Some(Some(error.clone())),
                        ..MeetingPatch::default()
                    };
                    let _ = self.ports.store.patch_meeting(meeting_id, patch).await;
                    self.pending.push_back(SessionEvent::RecoveryFailed { meeting_id, error });
                }
            },
        }
    }

    async fn start_capture(&mut self, meeting_id: MeetingId, app: Option<MeetingApp>, at: UnixMs) {
        let s = self.settings.clone();
        let title = default_title(app.as_ref());
        let new = NewMeeting {
            id: meeting_id,
            title: title.clone(),
            platform: app.as_ref().map_or(Platform::Other, |a| a.platform),
            app_bundle_id: app.as_ref().map(|a| a.bundle_id.clone()),
            started_at: at,
            language: acta_core::settings::language::normalize(&s.transcription.language),
        };
        if let Err(e) = self.ports.store.create_meeting(new).await {
            tracing::error!("no se pudo crear la reunión en la base de datos: {e}");
        }
        let config = SegmentWriterConfig {
            layout: self.ports.layout.clone(),
            meeting_id,
            format: AudioFormat::WORKING,
            segment_seconds: s.recording.segment_seconds,
            flush_every_ms: 1_000,
        };
        let mut writer = match SegmentWriter::create(config, at) {
            Ok(w) => w,
            Err(e) => {
                self.pending.push_back(SessionEvent::Fatal {
                    error: format!("No se pudo preparar la carpeta de la grabación: {e}"),
                    at,
                });
                return;
            }
        };
        writer.set_app(app.clone());
        self.next_gen += 1;
        let gen_id = self.next_gen;
        let live_session = (s.transcription.live_preview && self.ports.live_asr.is_some()).then(|| {
            let session = crate::live::LiveSession::new(meeting_id);
            writer.set_live_tap(session.tap.clone());
            lock(&self.ports.live_sessions).insert(meeting_id, session.clone());
            session
        });
        let watchdog = Watchdog::new(s.health, s.recording.segment_seconds, &Track::ALL, at);
        let core: SharedCore = Arc::new(Mutex::new(CaptureCore::new(writer, watchdog)));
        let mut autostop = AutoStopPolicy::new(s.auto_stop);
        let _ = autostop.on_tick(at);
        let mut rec = Recording {
            meeting_id,
            app: app.clone(),
            gen_id,
            title: title.clone(),
            core: core.clone(),
            autostop,
            pump: None,
            tasks: Vec::new(),
            speakers_on: false,
            screen_on: false,
            screen_retry: None,
            clipboard_on: false,
            live_on: false,
            screen_sharing: false,
            capture_count: 0,
        };

        let request = CaptureRequest {
            meeting_id,
            format: AudioFormat::WORKING,
            frame_ms: s.recording.frame_ms,
            mic: MicTarget {
                enabled: true,
                device_uid: s.recording.mic_device_uid.clone(),
            },
            system: system_target(app.as_ref()),
        };
        match self.ports.audio.start(request).await {
            Ok((_started, stream)) => {
                rec.pump = Some(spawn_pump(stream, core.clone(), self.ports.clock.clone(), self.tx.clone(), gen_id));
            }
            Err(e) => {
                if matches!(e, PortError::PermissionDenied(_)) {
                    lock(&core).watchdog.on_permission_missing(PermissionKind::Microphone, at);
                    let ws = lock(&core).watchdog.tick(at, None);
                    self.handle_warnings(ws);
                }
                self.rec = Some(rec);
                self.pending.push_back(SessionEvent::Fatal {
                    error: format!("No se pudo empezar a capturar el audio: {e}"),
                    at,
                });
                return;
            }
        }

        if let Some(app) = app.as_ref()
            && app.platform == Platform::Teams
        {
            match self.ports.speakers.watch(app).await {
                Ok(stream) => {
                    rec.speakers_on = true;
                    rec.tasks.push(forward(stream, self.tx.clone(), move |signal| Msg::Speaker { gen_id, signal }));
                }
                Err(e) => tracing::warn!("sin nombres de Teams (¿falta Accesibilidad?): {e}"),
            }
        }
        // Sin app detectada (grabación manual) se captura la pantalla principal.
        if s.screen.enabled && self.ports.screen.is_some() {
            let layout = &self.ports.layout;
            let req = ScreenCaptureRequest {
                meeting_id,
                app: app.clone(),
                out_dir: layout.captures_dir(meeting_id),
                candidates_dir: layout.capture_candidates_dir(meeting_id),
                fps: s.screen.fps,
                phash_threshold: s.screen.phash_threshold,
                stable_frames: s.screen.stable_frames,
                candidate_every_s: s.screen.candidate_every_s,
                ring_seconds: s.screen.ring_seconds,
                max_width: s.screen.max_width,
            };
            let _ = std::fs::create_dir_all(&req.candidates_dir);
            self.start_screen(&mut rec, req, at).await;
        }
        if let Some(clip) = self.ports.clipboard.clone() {
            match clip.start(self.ports.layout.captures_dir(meeting_id), s.screen.max_width).await {
                Ok(stream) => {
                    rec.clipboard_on = true;
                    rec.tasks.push(forward(stream, self.tx.clone(), move |frame| Msg::Clipboard { gen_id, frame }));
                }
                Err(e) => tracing::warn!("sin importar recortes del portapapeles: {e}"),
            }
        }
        if let (Some(session), Some(asr)) = (live_session, self.ports.live_asr.clone()) {
            let lang = acta_core::settings::language::asr_hint(&s.transcription.language);
            let tx = self.tx.clone();
            let on_update = move |u: crate::live::LiveUpdate| {
                for partial in u.partials {
                    let _ = tx.send(Msg::Live { gen_id, partial });
                }
                let _ = tx.send(Msg::LiveTranscript { gen_id, transcript: u.transcript });
            };
            let task = crate::live::run(session, asr, self.ports.layout.clone(), lang, crate::live::LiveConfig::default(), self.ports.clock.clone(), on_update);
            rec.tasks.push(tokio::spawn(task));
        } else if s.transcription.live_preview
            && let Some(live) = self.ports.live.clone()
        {
            let lang = acta_core::settings::language::asr_hint(&s.transcription.language);
            match live.start(&Track::ALL, lang).await {
                Ok(stream) => {
                    rec.live_on = true;
                    rec.tasks.push(forward(stream, self.tx.clone(), move |partial| Msg::Live { gen_id, partial }));
                }
                Err(e) => tracing::warn!("sin previsualización en vivo: {e}"),
            }
        }
        self.rec = Some(rec);
        self.emit(SupervisorEvent::MeetingUpdated(MeetingUpdated {
            meeting_id,
            status: MeetingStatus::Recording,
            title,
        }));
    }

    /// Arranca la captura de pantalla. Si falla, avisa (si es el permiso) y
    /// deja programado el reintento: TCC puede concederse a mitad de reunión
    /// y la ventana puede no existir todavía.
    async fn start_screen(&mut self, rec: &mut Recording, req: ScreenCaptureRequest, at: UnixMs) {
        let Some(screen) = self.ports.screen.clone() else { return };
        let gen_id = rec.gen_id;
        match screen.start(req.clone()).await {
            Ok(stream) => {
                rec.screen_on = true;
                rec.screen_retry = None;
                rec.tasks.push(forward(stream, self.tx.clone(), move |event| Msg::Screen { gen_id, event }));
                lock(&rec.core).watchdog.on_permission_granted(PermissionKind::ScreenRecording);
            }
            Err(e) => {
                if rec.screen_retry.is_none() {
                    tracing::warn!("sin capturas de pantalla (se reintenta cada {} s): {e}", SCREEN_RETRY_MS / 1000);
                } else {
                    tracing::debug!("reintento de la captura de pantalla: {e}");
                }
                if matches!(e, PortError::PermissionDenied(_)) {
                    lock(&rec.core).watchdog.on_permission_missing(PermissionKind::ScreenRecording, at);
                }
                rec.screen_retry = Some((req, at.plus_ms(SCREEN_RETRY_MS)));
            }
        }
    }

    async fn retry_screen(&mut self, now: UnixMs) {
        if matches!(self.state, SessionState::Paused { .. }) {
            return;
        }
        let Some(mut rec) = self.rec.take() else { return };
        if let Some((req, due)) = rec.screen_retry.clone()
            && now >= due
            && self.settings.screen.enabled
        {
            self.start_screen(&mut rec, req, now).await;
            if rec.screen_on {
                tracing::info!("la captura de pantalla ya funciona");
            }
        }
        self.rec = Some(rec);
    }

    async fn stop_capture(&mut self, meeting_id: MeetingId, reason: StopReason, at: UnixMs) {
        let Some(mut rec) = self.rec.take() else {
            self.pending.push_back(SessionEvent::CaptureFinalized { meeting_id });
            return;
        };
        if let Err(e) = self.ports.audio.stop().await {
            tracing::warn!("parada del audio: {e}");
        }
        if let Some(mut pump) = rec.pump.take()
            && tokio::time::timeout(self.config.stop_drain, &mut pump).await.is_err()
        {
            tracing::warn!("el audio no terminó a tiempo; se corta");
            pump.abort();
        }
        for t in rec.tasks.drain(..) {
            t.abort();
        }
        if rec.speakers_on {
            let _ = self.ports.speakers.unwatch().await;
        }
        if rec.screen_on
            && let Some(screen) = &self.ports.screen
        {
            let _ = screen.stop().await;
        }
        if rec.clipboard_on
            && let Some(clip) = &self.ports.clipboard
        {
            let _ = clip.stop().await;
        }
        if rec.live_on
            && let Some(live) = &self.ports.live
        {
            let _ = live.stop().await;
        }
        if rec.screen_sharing {
            let _ = append_jsonl(
                &self.ports.layout.live_screen_path(meeting_id),
                &ScreenEvent::ShareEnded { at },
            );
        }
        let writer = lock(&rec.core).writer.take();
        if reason == StopReason::Discarded {
            drop(writer);
            if let Err(e) = self.ports.store.delete_meeting(meeting_id).await {
                tracing::error!("no se pudo borrar la reunión descartada: {e}");
            }
            let dir = self.ports.layout.meeting_dir(meeting_id);
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::error!("no se pudo borrar la carpeta descartada ({}): {e}", dir.display());
            }
            self.clear_warnings();
            self.pending.push_back(SessionEvent::CaptureFinalized { meeting_id });
            return;
        }
        let duration = match writer.map(|w| w.finish_reporting(reason, at)) {
            Some(Ok((manifest, failures))) => {
                for f in failures {
                    tracing::error!("fallo al cerrar la grabación ({}): {}", f.track.as_str(), f.message);
                }
                manifest_duration(&manifest)
            }
            Some(Err(e)) => {
                tracing::error!("no se pudo cerrar el manifiesto: {e}");
                MediaMs(0)
            }
            None => MediaMs(0),
        };
        let patch = MeetingPatch {
            ended_at: Some(at),
            duration: Some(duration),
            status: Some(MeetingStatus::Processing),
            ..MeetingPatch::default()
        };
        let title = match self.ports.store.patch_meeting(meeting_id, patch).await {
            Ok(m) => m.title,
            Err(e) => {
                tracing::error!("no se pudo actualizar la reunión: {e}");
                rec.title.clone()
            }
        };
        self.emit(SupervisorEvent::MeetingUpdated(MeetingUpdated {
            meeting_id,
            status: MeetingStatus::Processing,
            title,
        }));
        self.clear_warnings();
        self.pending.push_back(SessionEvent::CaptureFinalized { meeting_id });
    }

    /// Repara los segmentos y deja la reunión lista para procesar.
    async fn recover_meeting(&mut self, id: MeetingId) -> PortResult<()> {
        let layout = self.ports.layout.clone();
        let manifest = tokio::task::spawn_blocking(move || recover(&layout, id))
            .await
            .map_err(|e| PortError::Internal(e.to_string()))??;
        let store = &self.ports.store;
        if store.get_meeting(id).await?.is_none() {
            let new = NewMeeting {
                id,
                title: format!("{} (recuperada)", default_title(manifest.app.as_ref())),
                platform: manifest.app.as_ref().map_or(Platform::Other, |a| a.platform),
                app_bundle_id: manifest.app.as_ref().map(|a| a.bundle_id.clone()),
                started_at: manifest.created_at,
                language: acta_core::settings::language::normalize(&self.settings.transcription.language),
            };
            store.create_meeting(new).await?;
        }
        let patch = MeetingPatch {
            ended_at: Some(manifest.updated_at),
            duration: Some(manifest_duration(&manifest)),
            status: Some(MeetingStatus::Processing),
            recovered: Some(true),
            error: Some(None),
            ..MeetingPatch::default()
        };
        let m = store.patch_meeting(id, patch).await?;
        self.emit(SupervisorEvent::MeetingUpdated(MeetingUpdated {
            meeting_id: id,
            status: MeetingStatus::Processing,
            title: m.title,
        }));
        Ok(())
    }
}

fn system_target(app: Option<&MeetingApp>) -> SystemTarget {
    match app {
        Some(a) => SystemTarget {
            enabled: true,
            pids: if a.audio_pids.is_empty() { vec![a.pid] } else { a.audio_pids.clone() },
            bundle_ids: vec![a.bundle_id.clone()],
        },
        // Grabación manual: tap global de la salida.
        None => SystemTarget {
            enabled: true,
            pids: vec![],
            bundle_ids: vec![],
        },
    }
}
