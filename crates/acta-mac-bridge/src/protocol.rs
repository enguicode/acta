//! Protocolo sidecar ⇄ Rust, versión 1. Espejo EXACTO de
//! `sidecars/acta-mac/Sources/ActaMacProtocol/Protocol.swift`.
//! Especificación: docs/01-arquitectura.md §4.
//!
//! Transporte: JSON por líneas (UTF-8, `\n`), una línea ≤ 1 MiB.
//! - stdin  (Rust → sidecar): `{"id":7,"cmd":"capture.start","args":{...}}`
//! - stdout (sidecar → Rust): respuestas `{"type":"response","id":7,"ok":true,"result":{...}}`
//!   o `{"type":"response","id":7,"ok":false,"error":{"code":"...","message":"..."}}`,
//!   y eventos `{"type":"event","ts":1731750000123,"event":"audio.frame","data":{...}}`.
//! - stderr: logs del sidecar en texto libre (nunca protocolo).
//!
//! Claves en snake_case (Swift: `.convertToSnakeCase` / `.convertFromSnakeCase`).
//! Los comandos sin argumentos se envían sin `args`.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Id del modelo de embeddings de voz (huella de voz y diarización). Las huellas
/// solo son comparables entre sí si comparten este id. Espejo de
/// `speakerEmbeddingModel` en el sidecar Swift.
pub const SPEAKER_EMBEDDING_MODEL: &str = "fluidaudio-wespeaker-community1-256";
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

// ===========================================================================
// Rust → sidecar
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args")]
pub enum Command {
    #[serde(rename = "hello")]
    Hello(HelloArgs),
    #[serde(rename = "shutdown")]
    Shutdown,
    #[serde(rename = "permissions.status")]
    PermissionsStatus,
    #[serde(rename = "permissions.request")]
    PermissionsRequest(PermissionsRequestArgs),
    #[serde(rename = "devices.list")]
    DevicesList,
    #[serde(rename = "apps.list")]
    AppsList,
    #[serde(rename = "detect.start")]
    DetectStart(DetectStartArgs),
    #[serde(rename = "detect.stop")]
    DetectStop,
    #[serde(rename = "capture.start")]
    CaptureStart(CaptureStartArgs),
    #[serde(rename = "capture.pause")]
    CapturePause,
    #[serde(rename = "capture.resume")]
    CaptureResume,
    #[serde(rename = "capture.stop")]
    CaptureStop,
    #[serde(rename = "capture.retarget")]
    CaptureRetarget(CaptureRetargetArgs),
    #[serde(rename = "speakers.watch")]
    SpeakersWatch(SpeakersWatchArgs),
    #[serde(rename = "speakers.unwatch")]
    SpeakersUnwatch,
    #[serde(rename = "screen.start")]
    ScreenStart(ScreenStartArgs),
    #[serde(rename = "screen.snapshot")]
    ScreenSnapshot(ScreenSnapshotArgs),
    #[serde(rename = "screen.stop")]
    ScreenStop,
    /// Vigila el portapapeles: cada imagen nueva → `clipboard.image`.
    #[serde(rename = "clipboard.start")]
    ClipboardStart(ClipboardStartArgs),
    #[serde(rename = "clipboard.stop")]
    ClipboardStop,
    #[serde(rename = "asr.transcribe")]
    AsrTranscribe(AsrTranscribeArgs),
    #[serde(rename = "asr.live.start")]
    AsrLiveStart(AsrLiveStartArgs),
    #[serde(rename = "asr.live.stop")]
    AsrLiveStop,
    #[serde(rename = "diarize.run")]
    DiarizeRun(DiarizeRunArgs),
    #[serde(rename = "voiceprint.embed")]
    VoiceprintEmbed(VoiceprintEmbedArgs),
    #[serde(rename = "models.status")]
    ModelsStatus,
    #[serde(rename = "models.download")]
    ModelsDownload(ModelsDownloadArgs),
    #[serde(rename = "job.cancel")]
    JobCancel(JobCancelArgs),
    /// Títulos de las ventanas de la app de la reunión (AX si hay
    /// Accesibilidad, si no ScreenCaptureKit). Rust elige el bueno.
    #[serde(rename = "meeting.titles")]
    MeetingTitles(MeetingTitlesArgs),
    /// Eventos del Calendario de macOS (EventKit) en un intervalo.
    #[serde(rename = "calendar.events")]
    CalendarEvents(CalendarEventsArgs),
    /// Nombre de la cuenta e idiomas preferidos de macOS (para la bienvenida).
    #[serde(rename = "system.profile")]
    SystemProfile,
    /// Apps instaladas y en marcha, con su icono en PNG.
    #[serde(rename = "apps.installed")]
    AppsInstalled(AppsInstalledArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemProfileResult {
    /// `NSFullUserName()`. Falta si está vacío.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    /// `Locale.preferredLanguages`, en orden (`"es-ES"`).
    pub preferred_languages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsInstalledArgs {
    /// Carpeta donde dejar `<bundle id>.png` (se crea; no se reescribe un
    /// icono que ya está y es más nuevo que la app).
    pub icons_dir: String,
    /// Lado del PNG en píxeles.
    pub icon_px: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledAppWire {
    pub bundle_id: String,
    pub name: String,
    /// Ruta del `.app`.
    pub path: String,
    pub running: bool,
    /// Ruta absoluta del PNG. Falta si no se pudo sacar el icono.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsInstalledResult {
    pub apps: Vec<InstalledAppWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingTitlesArgs {
    pub pid: i32,
    pub bundle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingTitlesResult {
    pub titles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEventsArgs {
    pub from_unix_ms: i64,
    pub to_unix_ms: i64,
    /// Pedir el permiso si está sin decidir (solo con el ajuste activado).
    pub request_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub title: String,
    pub start_unix_ms: i64,
    pub end_unix_ms: i64,
    pub all_day: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEventsResult {
    /// `granted` | `denied` | `not_determined`
    pub access: String,
    pub events: Vec<CalendarEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackName {
    Mic,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionName {
    Microphone,
    SystemAudio,
    ScreenRecording,
    Accessibility,
    Automation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatusName {
    Granted,
    Denied,
    NotDetermined,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloArgs {
    pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsRequestArgs {
    pub kind: PermissionName,
    /// Solo `automation`: bundle id del navegador.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRuleWire {
    pub bundle_id: String,
    /// "teams" | "meet" | "zoom" | "slack" | "other"
    pub platform: String,
    pub is_browser: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectStartArgs {
    pub rules: Vec<AppRuleWire>,
    pub meet_url_patterns: Vec<String>,
    pub poll_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicArgs {
    pub enabled: bool,
    /// `null` = dispositivo por defecto y seguirlo si cambia.
    pub device_uid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemArgs {
    pub enabled: bool,
    pub pids: Vec<i32>,
    pub bundle_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureStartArgs {
    pub session_id: String,
    /// Siempre 16000.
    pub sample_rate: u32,
    /// Siempre 100.
    pub frame_ms: u32,
    pub mic: MicArgs,
    pub system: SystemArgs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRetargetArgs {
    pub pids: Vec<i32>,
    pub bundle_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakersWatchArgs {
    pub pid: i32,
    pub bundle_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenStartArgs {
    pub session_id: String,
    /// 0 = sin app: la pantalla principal (sin las ventanas de acta).
    pub pid: i32,
    pub bundle_id: String,
    pub window_title_hint: Option<String>,
    pub out_dir: String,
    pub candidates_dir: String,
    pub fps: f32,
    pub phash_threshold: u32,
    pub stable_frames: u32,
    pub candidate_every_s: u32,
    pub ring_seconds: u32,
    pub max_width: u32,
    /// "jpeg" (calidad 0.8).
    pub format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureReasonName {
    SlideChange,
    Keyword,
    LlmPick,
    Manual,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardStartArgs {
    /// `captures/` de la reunión (`<unix_ms>-manual.jpg`).
    pub out_dir: String,
    pub max_width: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSnapshotArgs {
    pub reason: CaptureReasonName,
    pub at_unix_ms: i64,
    pub keyword: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrTranscribeArgs {
    pub job_id: String,
    pub wav_path: String,
    /// El sidecar escribe aquí un `AsrFile` (vía `.tmp` + rename).
    pub out_path: String,
    pub language: Option<String>,
    /// "parakeet-tdt-0.6b-v3"
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrLiveStartArgs {
    pub tracks: Vec<TrackName>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiarizeRunArgs {
    pub job_id: String,
    pub wav_path: String,
    /// El sidecar escribe aquí un `DiarizationFile`.
    pub out_path: String,
    pub num_speakers: Option<u8>,
    pub min_speakers: Option<u8>,
    pub max_speakers: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanMs {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceprintEmbedArgs {
    pub wav_path: String,
    pub spans: Vec<SpanMs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsDownloadArgs {
    /// "asr" | "diarizer" | "vad"
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCancelArgs {
    pub job_id: String,
}

// ===========================================================================
// sidecar → Rust
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Incoming {
    Response(Response),
    Event(EventEnvelope),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    PermissionDenied,
    NotFound,
    Unsupported,
    InvalidArgs,
    Busy,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Reloj de pared del sidecar (unix ms) al emitir.
    pub ts: i64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum Event {
    #[serde(rename = "audio.frame")]
    AudioFrame(AudioFrameEvent),
    #[serde(rename = "audio.device_changed")]
    AudioDeviceChanged(AudioDeviceChangedEvent),
    #[serde(rename = "audio.track_state")]
    AudioTrackState(AudioTrackStateEvent),
    #[serde(rename = "audio.overrun")]
    AudioOverrun(AudioOverrunEvent),
    #[serde(rename = "detect.signal")]
    DetectSignal(DetectSignalEvent),
    #[serde(rename = "speakers.roster")]
    SpeakersRoster(SpeakersRosterEvent),
    #[serde(rename = "speakers.active")]
    SpeakersActive(SpeakersActiveEvent),
    #[serde(rename = "screen.share_state")]
    ScreenShareState(ScreenShareStateEvent),
    #[serde(rename = "screen.frame")]
    ScreenFrame(ScreenFrameEvent),
    /// Imagen copiada al portapapeles, ya guardada como JPEG (`reason: manual`).
    #[serde(rename = "clipboard.image")]
    ClipboardImage(ScreenFrameEvent),
    #[serde(rename = "asr.partial")]
    AsrPartial(AsrPartialEvent),
    #[serde(rename = "job.progress")]
    JobProgress(JobProgressEvent),
    #[serde(rename = "models.progress")]
    ModelsProgress(ModelsProgressEvent),
    #[serde(rename = "permissions.changed")]
    PermissionsChanged(PermissionsChangedEvent),
    #[serde(rename = "log")]
    Log(LogEvent),
    #[serde(rename = "fatal")]
    Fatal(FatalEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFrameEvent {
    pub track: TrackName,
    /// Secuencia por pista desde 0.
    pub seq: u64,
    /// µs desde `capture.start` (reloj host común a ambas pistas).
    pub pts_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    /// Siempre "s16le".
    pub format: String,
    /// PCM base64 (estándar, con padding).
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceChangedEvent {
    pub track: TrackName,
    pub device_uid: Option<String>,
    pub device_name: Option<String>,
    /// "default_input_changed" | "default_output_changed" | "device_removed" | "route_changed"
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackStateName {
    Running,
    Stalled,
    Restarting,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrackStateEvent {
    pub track: TrackName,
    pub state: TrackStateName,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOverrunEvent {
    pub track: TrackName,
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalName {
    MicInUse,
    MicReleased,
    MeetingWindowOpen,
    MeetingWindowClosed,
    MeetTabOpen,
    MeetTabClosed,
    AudioPidsChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfoWire {
    pub pid: i32,
    pub bundle_id: String,
    pub name: String,
    /// PIDs que producen audio para esta app (helpers incluidos).
    #[serde(default)]
    pub audio_pids: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectSignalEvent {
    pub signal: SignalName,
    pub app: AppInfoWire,
    pub window_title: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantWire {
    pub name: String,
    pub is_self: bool,
    pub muted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakersRosterEvent {
    pub pid: i32,
    pub participants: Vec<ParticipantWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakersActiveEvent {
    pub pid: i32,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenShareStateEvent {
    pub sharing: bool,
    pub presenter: Option<String>,
    /// "ax" | "frame"
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrameEvent {
    /// Ruta absoluta del JPEG ya escrito (tmp + rename).
    pub path: String,
    pub at_unix_ms: i64,
    pub reason: CaptureReasonName,
    /// dHash 64 bits en hex (16 chars).
    pub phash: String,
    pub width: u32,
    pub height: u32,
    pub keyword: Option<String>,
    /// Miniatura gris 64×36 en hex (4608 chars). Ausente en sidecars viejos.
    #[serde(default)]
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrPartialEvent {
    pub track: TrackName,
    pub text: String,
    pub at_unix_ms: i64,
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProgressEvent {
    pub job_id: String,
    pub fraction: f32,
    pub stage: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsProgressEvent {
    pub name: String,
    pub fraction: f32,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsChangedEvent {
    pub kind: PermissionName,
    pub status: PermissionStatusName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    /// "debug" | "info" | "warn" | "error"
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FatalEvent {
    pub message: String,
}

// ===========================================================================
// Resultados (campo `result` de la respuesta)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub sidecar_version: String,
    pub protocol_version: u32,
    pub macos_version: String,
    /// p. ej. ["process_tap", "screen_capture", "asr", "diarization", "ax_teams", "live_asr"]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionEntry {
    pub kind: PermissionName,
    pub status: PermissionStatusName,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsStatusResult {
    pub permissions: Vec<PermissionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsRequestResult {
    pub status: PermissionStatusName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceWire {
    pub uid: String,
    pub name: String,
    pub is_default: bool,
    /// "builtin" | "bluetooth" | "usb" | "virtual" | "other"
    pub transport: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesListResult {
    pub inputs: Vec<DeviceWire>,
    pub outputs: Vec<DeviceWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsListResult {
    pub apps: Vec<AppInfoWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureStartResult {
    /// Unix ms correspondiente a `pts_us = 0`.
    pub started_at_unix_ms: i64,
    pub mic_device: Option<String>,
    pub system_pids: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSnapshotResult {
    pub frame: Option<ScreenFrameEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrTranscribeResult {
    pub out_path: String,
    pub segments: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiarizeRunResult {
    pub out_path: String,
    pub speakers: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceprintEmbedResult {
    pub embedding: Vec<f32>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelStatusWire {
    pub name: String,
    pub downloaded: bool,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsStatusResult {
    pub models: Vec<ModelStatusWire>,
}

// ===========================================================================
// Ficheros de salida de trabajos largos
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrWordWire {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrSegmentWire {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence: Option<f32>,
    #[serde(default)]
    pub words: Vec<AsrWordWire>,
}

/// Contenido de `transcript/asr-<track>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrFile {
    pub model: String,
    pub language: Option<String>,
    pub duration_ms: u64,
    pub segments: Vec<AsrSegmentWire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationTurnWire {
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizedSpeakerWire {
    pub label: String,
    pub embedding: Vec<f32>,
    pub total_ms: u64,
}

/// Contenido de `transcript/diarization.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationFile {
    pub embedding_model: String,
    pub turns: Vec<DiarizationTurnWire>,
    pub speakers: Vec<DiarizedSpeakerWire>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn speaker_embedding_model_id_is_stable() {
        assert_eq!(SPEAKER_EMBEDDING_MODEL, "fluidaudio-wespeaker-community1-256");
    }

    #[test]
    fn request_with_args_has_expected_shape() {
        let req = Request {
            id: 7,
            command: Command::CaptureRetarget(CaptureRetargetArgs {
                pids: vec![1, 2],
                bundle_ids: vec!["com.microsoft.teams2".into()],
            }),
        };
        let v = serde_json::to_value(&req).expect("ser");
        assert_eq!(
            v,
            json!({"id": 7, "cmd": "capture.retarget", "args": {"pids": [1, 2], "bundle_ids": ["com.microsoft.teams2"]}})
        );
        let back: Request = serde_json::from_value(v).expect("de");
        assert_eq!(back, req);
    }

    #[test]
    fn unit_command_has_no_args() {
        let req = Request {
            id: 1,
            command: Command::CaptureStop,
        };
        let v = serde_json::to_value(&req).expect("ser");
        assert_eq!(v, json!({"id": 1, "cmd": "capture.stop"}));
        let back: Request = serde_json::from_str(r#"{"id":1,"cmd":"capture.stop"}"#).expect("de");
        assert_eq!(back, req);
    }

    #[test]
    fn parses_audio_frame_event_line() {
        let line = r#"{"type":"event","ts":1731750000123,"event":"audio.frame","data":{"track":"system","seq":42,"pts_us":4200000,"sample_rate":16000,"channels":1,"format":"s16le","data":"AAA="}}"#;
        let msg: Incoming = serde_json::from_str(line).expect("parse");
        match msg {
            Incoming::Event(EventEnvelope {
                ts,
                event: Event::AudioFrame(f),
            }) => {
                assert_eq!(ts, 1_731_750_000_123);
                assert_eq!(f.track, TrackName::System);
                assert_eq!(f.seq, 42);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn screen_frame_thumbnail_is_optional_for_older_sidecars() {
        let old = r#"{"path":"/p","at_unix_ms":1,"reason":"manual","phash":"00","width":1,"height":1,"keyword":null}"#;
        assert_eq!(serde_json::from_str::<ScreenFrameEvent>(old).unwrap().thumb, None);
        let new = r#"{"path":"/p","at_unix_ms":1,"reason":"slide_change","phash":"00","width":1,"height":1,"keyword":null,"thumb":"0a0b"}"#;
        let ev: ScreenFrameEvent = serde_json::from_str(new).unwrap();
        assert_eq!(ev.thumb.as_deref(), Some("0a0b"));
        assert_eq!(crate::screen::frame_from_wire(ev).thumb.as_deref(), Some("0a0b"));
    }

    #[test]
    fn parses_ok_and_error_responses() {
        let ok: Incoming =
            serde_json::from_str(r#"{"type":"response","id":3,"ok":true,"result":{"out_path":"/x","speakers":2}}"#)
                .expect("ok");
        let Incoming::Response(r) = ok else {
            panic!("not a response")
        };
        let res: DiarizeRunResult = serde_json::from_value(r.result.expect("result")).expect("typed");
        assert_eq!(res.speakers, 2);

        let err: Incoming = serde_json::from_str(
            r#"{"type":"response","id":4,"ok":false,"error":{"code":"permission_denied","message":"tcc"}}"#,
        )
        .expect("err");
        let Incoming::Response(r) = err else {
            panic!("not a response")
        };
        assert_eq!(r.error.expect("error").code, ErrorCode::PermissionDenied);
    }

    #[test]
    fn parses_detect_signal() {
        let line = r#"{"type":"event","ts":1,"event":"detect.signal","data":{"signal":"mic_in_use","app":{"pid":10,"bundle_id":"com.microsoft.teams2","name":"Microsoft Teams","audio_pids":[10,11]},"window_title":null,"url":null}}"#;
        let msg: Incoming = serde_json::from_str(line).expect("parse");
        assert!(matches!(
            msg,
            Incoming::Event(EventEnvelope {
                event: Event::DetectSignal(DetectSignalEvent {
                    signal: SignalName::MicInUse,
                    ..
                }),
                ..
            })
        ));
    }
}
