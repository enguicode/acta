//! Supervisor de sesión: dueño ÚNICO del `SessionState`. Recibe
//! `SessionEvent` (de la UI, del detector, de la autoparada, del writer),
//! aplica `transition` y ejecuta los `Effect`. Publica `SupervisorEvent`
//! que la app reenvía como eventos Tauri.
//!
//! Piezas:
//! - `actor`: el bucle (un único `tokio::spawn`), estado y efectos.
//! - `capture`: writer + watchdog + medidores de la grabación en curso, y la
//!   tarea que vacía el stream de audio en ellos.
//! - `queue`: cola de procesado en segundo plano (no bloquea grabar otra).
//! - `files`: señales en vivo (`live/*.jsonl`) y colocación de capturas.
//!
//! Este módulo no lanza procesos: los adaptadores (sidecar, CLI) lo hacen
//! supervisados en sus crates.

mod actor;
mod capture;
mod gate;
pub mod files;
mod queue;

use acta_core::api::{MeetingUpdated, SessionSnapshot};
use acta_core::detection::{DetectionEvent, MeetingDetector, SpeakerNameSource, SpeakerSignal};
use acta_core::ids::MeetingId;
use acta_core::layout::DataLayout;
use acta_core::library::MeetingStore;
use acta_core::minutes::ImportantMomentDetector;
use acta_core::processing::{ProcessingPipeline, ProcessingProgress, ProcessingStep};
use acta_core::recording::{AudioSource, HealthWarning, Levels, SessionEvent};
use acta_core::screen::{Capture, CapturedFrame, ClipboardImporter, ScreenCapturer, ScreenEvent};
use acta_core::settings::Settings;
use acta_core::time::Clock;
use acta_core::transcription::{LiveTranscriber, LivePartial, Transcriber};
use acta_core::{PortError, PortResult};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Lo que publica el supervisor (la app → `app.emit`).
#[derive(Debug, Clone)]
pub enum SupervisorEvent {
    /// → `session:state`
    Snapshot(SessionSnapshot),
    /// → `session:levels`
    Levels(Levels),
    /// → `session:health`
    Health(HealthWarning),
    /// → `capture:saved`
    CaptureSaved(Capture),
    /// → `processing:progress`
    Processing(ProcessingProgress),
    /// → `meeting:updated`
    MeetingUpdated(MeetingUpdated),
    /// → `live:partial`
    LivePartial(LivePartial),
    /// → `live:transcript` (segmentos nuevos o cambiados)
    LiveTranscript(acta_core::api::LiveTranscript),
}

/// Sesiones de transcripción en vivo por reunión (la app las lee para
/// `live_transcript_get` y `live_ask`).
pub type LiveSessions = Arc<std::sync::Mutex<std::collections::HashMap<MeetingId, Arc<crate::live::LiveSession>>>>;

/// Espacio libre (bytes) del volumen que contiene la ruta. Lo mide la app
/// (necesita una llamada al sistema); `None` = desconocido.
pub type DiskFree = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// Puertos que necesita el supervisor (los inyecta la app).
#[derive(Clone)]
pub struct SupervisorPorts {
    pub layout: DataLayout,
    pub clock: Arc<dyn Clock>,
    pub audio: Arc<dyn AudioSource>,
    pub detector: Arc<dyn MeetingDetector>,
    pub speakers: Arc<dyn SpeakerNameSource>,
    pub screen: Option<Arc<dyn ScreenCapturer>>,
    /// Recortes que el usuario copia al portapapeles durante la grabación.
    pub clipboard: Option<Arc<dyn ClipboardImporter>>,
    pub live: Option<Arc<dyn LiveTranscriber>>,
    /// ASR para la transcripción en vivo por ventanas (`crate::live`). Si
    /// está, sustituye a `live` (no se transcribe dos veces).
    pub live_asr: Option<Arc<dyn Transcriber>>,
    pub live_sessions: LiveSessions,
    pub moments: Arc<dyn ImportantMomentDetector>,
    pub store: Arc<dyn MeetingStore>,
    pub pipeline: Arc<dyn ProcessingPipeline>,
    pub disk_free: DiskFree,
}

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Periodo del tick (autoparada, watchdog, snapshot de 1 Hz, caducidad).
    pub tick: Duration,
    /// Cuánto se espera a que el audio vacíe sus últimos frames al parar.
    pub stop_drain: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(1),
            stop_drain: Duration::from_secs(2),
        }
    }
}

pub(crate) enum Request {
    Event(SessionEvent, oneshot::Sender<PortResult<SessionSnapshot>>),
    Snapshot(oneshot::Sender<SessionSnapshot>),
    Health(oneshot::Sender<Vec<HealthWarning>>),
    UpdateSettings(Box<Settings>),
    Reprocess(MeetingId, Vec<ProcessingStep>),
    CaptureManual(oneshot::Sender<PortResult<Option<Capture>>>),
    Shutdown(oneshot::Sender<()>),
}

/// Mensajes que recibe el bucle. `gen_id` identifica la grabación: los
/// mensajes de una grabación anterior se descartan.
pub(crate) enum Msg {
    Req(Request),
    Levels { gen_id: u64, levels: Levels },
    AudioEnded { gen_id: u64, error: Option<String> },
    Detection(DetectionEvent),
    Speaker { gen_id: u64, signal: SpeakerSignal },
    Screen { gen_id: u64, event: ScreenEvent },
    Clipboard { gen_id: u64, frame: CapturedFrame },
    Live { gen_id: u64, partial: LivePartial },
    LiveTranscript { gen_id: u64, transcript: acta_core::api::LiveTranscript },
    Processing(SessionEvent),
}

/// Handle clonable que usan los comandos Tauri.
#[derive(Clone)]
pub struct SupervisorHandle {
    tx: mpsc::UnboundedSender<Msg>,
    events: broadcast::Sender<SupervisorEvent>,
}

fn gone() -> PortError {
    PortError::Unavailable("el supervisor de sesión no está en marcha".into())
}

impl SupervisorHandle {
    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Request) -> PortResult<T> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Msg::Req(make(tx))).map_err(|_| gone())?;
        rx.await.map_err(|_| gone())
    }

    /// Envía un evento y devuelve el snapshot resultante. Un evento que no
    /// vale en el estado actual devuelve `InvalidInput` y no cambia nada.
    pub async fn send(&self, event: SessionEvent) -> PortResult<SessionSnapshot> {
        self.ask(|tx| Request::Event(event, tx)).await?
    }

    pub async fn snapshot(&self) -> PortResult<SessionSnapshot> {
        self.ask(Request::Snapshot).await
    }

    /// Avisos de salud activos.
    pub async fn health(&self) -> PortResult<Vec<HealthWarning>> {
        self.ask(Request::Health).await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SupervisorEvent> {
        self.events.subscribe()
    }

    /// Ajustes nuevos (detección se reinicia si cambió).
    pub fn update_settings(&self, settings: Settings) -> PortResult<()> {
        self.tx
            .send(Msg::Req(Request::UpdateSettings(Box::new(settings))))
            .map_err(|_| gone())
    }

    /// Encola un reprocesado (`steps` vacío = todos desde `Transcribing`).
    pub fn reprocess(&self, meeting_id: MeetingId, steps: Vec<ProcessingStep>) -> PortResult<()> {
        self.tx
            .send(Msg::Req(Request::Reprocess(meeting_id, steps)))
            .map_err(|_| gone())
    }

    /// Captura manual de la pantalla compartida (si la captura está activa).
    pub async fn capture_manual(&self) -> PortResult<Option<Capture>> {
        self.ask(Request::CaptureManual).await?
    }

    /// Para la grabación en curso (`AppQuit`) y el detector. Idempotente.
    pub async fn shutdown(&self) -> PortResult<()> {
        match self.ask(Request::Shutdown).await {
            Ok(()) | Err(PortError::Unavailable(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Arranca el supervisor: recuperación de sesiones incompletas, reuniones a
/// medio procesar, detector y bucle de eventos.
pub async fn spawn(ports: SupervisorPorts, settings: Settings) -> PortResult<SupervisorHandle> {
    spawn_with(ports, settings, SupervisorConfig::default()).await
}

pub async fn spawn_with(
    ports: SupervisorPorts,
    settings: Settings,
    config: SupervisorConfig,
) -> PortResult<SupervisorHandle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let (events, _) = broadcast::channel(1024);
    let mut actor = actor::Actor::new(ports, settings, config, tx.clone(), events.clone());
    actor.startup().await;
    tokio::spawn(actor.run(rx));
    Ok(SupervisorHandle { tx, events })
}
