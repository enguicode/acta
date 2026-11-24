//! Comandos Tauri: nombre → (argumentos, resultado).
//!
//! | comando | args (JS, camelCase) | resultado |
//! |---|---|---|
//! | `session_get_state` | — | `SessionSnapshot` |
//! | `session_start` | `{ app?: MeetingApp }` | `SessionSnapshot` |
//! | `session_pause` / `session_resume` / `session_stop` / `session_dismiss` | — | `SessionSnapshot` |
//! | `prompt_answer` | `{ promptId, answer }` | `SessionSnapshot` |
//! | `capture_manual` | — | `Capture \| null` |
//! | `meetings_list` | `{ query: MeetingQuery }` | `Page<MeetingSummary>` |
//! | `meeting_get` | `{ id }` | `MeetingDetail` |
//! | `meeting_update` | `{ id, title }` | `Meeting` |
//! | `meeting_set_note` | `{ id, note }` | `Meeting` (nota personal; vacía la borra) |
//! | `meeting_delete` | `{ id }` | `null` |
//! | `meeting_set_favourite` | `{ id, favourite }` | `Meeting` |
//! | `group_set_favourite` | `{ id, favourite }` | `Group` |
//! | `cleanup_suggestions` | — | `CleanupSuggestions` (vacío si están apagadas) |
//! | `cleanup_delete` | `{ ids }` | `CleanupResult` (nunca borra favoritas) |
//! | `cleanup_keep` | `{ ids }` | `null` («Mantener»: no se vuelven a sugerir) |
//! | `meeting_reprocess` | `{ request: ReprocessRequest }` | `null` |
//! | `speaker_rename` | `{ meetingId, speakerId, displayName, remember }` | `null` |
//! | `action_item_set_done` | `{ id, done }` | `null` |
//! | `capture_delete` | `{ id }` | `null` |
//! | `groups_list` | — | `Group[]` |
//! | `group_create` | `{ name, description? }` | `Group` |
//! | `group_update` | `{ id, name?, description? }` | `Group` |
//! | `group_delete` | `{ id }` | `null` |
//! | `group_add_meeting` / `group_remove_meeting` | `{ groupId, meetingId }` | `null` |
//! | `group_suggestions_list` | — | `GroupSuggestion[]` |
//! | `group_suggestions_refresh` | — | `GroupSuggestion[]` |
//! | `group_suggestion_accept` | `{ id, name? }` | `Group` |
//! | `group_suggestion_dismiss` | `{ id }` | `null` |
//! | `search` | `{ query: SearchQuery }` | `SearchHit[]` |
//! | `chat_threads_list` | `{ scope?: ChatScope }` | `ChatThread[]` |
//! | `chat_thread_create` | `{ scope: ChatScope }` | `ChatThread` |
//! | `chat_thread_delete` | `{ threadId }` | `null` |
//! | `chat_messages_list` | `{ threadId }` | `ChatMessage[]` |
//! | `chat_send` | `{ threadId, text, providerId?, model? }` | `ChatSendResult` (+ eventos `chat:delta`; proveedor/modelo solo para esa respuesta) |
//! | `voiceprints_list` | — | `VoiceprintSummary[]` |
//! | `voiceprint_delete` | `{ id }` | `null` |
//! | `settings_get` | — | `Settings` |
//! | `settings_update` | `{ settings }` | `Settings` |
//! | `llm_provider_test` | `{ providerId }` | `ProviderTestResult` |
//! | `llm_cli_detect` | — | `CliDetection` |
//! | `secret_set` | `{ providerId, value }` | `SecretRef` (Llavero; nunca se guarda en SQLite) |
//! | `secret_delete` | `{ providerId }` | `null` |
//! | `local_llm_status` | — | `LocalLlmStatus` (hardware, runtime, niveles y recomendación) |
//! | `local_llm_download` | `{ modelId }` | `null` (+ eventos `localmodel:download`; reanuda) |
//! | `local_llm_cancel` | `{ modelId }` | `null` (la descarga queda a medias y se puede reanudar) |
//! | `local_llm_delete` | `{ modelId }` | `null` |
//! | `local_llm_test` | `{ modelId }` | `ProviderTestResult` |
//! | `models_status` | — | `ModelStatus[]` |
//! | `audio_inputs_list` | — | `AudioInput[]` (micros para `Settings.recording.micDeviceUid`) |
//! | `models_download` | `{ name }` (`asr` \| `diarizer` \| `vad`) | `null` (+ eventos `model:download`) |
//! | `permissions_status` | — | `PermissionState[]` |
//! | `permissions_request` | `{ kind }` | `PermissionStatus` |
//! | `open_system_settings` | `{ kind }` | `null` |
//! | `health_get` | — | `HealthWarning[]` |
//! | `app_info` | — | `AppInfo` |
//! | `overlay_set_expanded` | `{ expanded }` | `null` |
//! | `main_window_show` | `{ meetingId? }` | `null` |
//!
//! Ola 4 (en vivo, reintento con otro proveedor, búsqueda en el transcript,
//! título de grupos): ver [`super::live`]. Ola 5 (`system_profile`,
//! `apps_list`): ver [`super::system`].

use crate::chat::ChatMessage;
use crate::ids::ChatMessageId;
use crate::recording::{HealthWarning, SessionState};
use crate::time::UnixMs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Nombres de comando (para no teclear strings sueltos en Rust).
pub mod command_names {
    pub const SESSION_GET_STATE: &str = "session_get_state";
    pub const SESSION_START: &str = "session_start";
    pub const SESSION_PAUSE: &str = "session_pause";
    pub const SESSION_RESUME: &str = "session_resume";
    pub const SESSION_STOP: &str = "session_stop";
    pub const SESSION_DISMISS: &str = "session_dismiss";
    pub const PROMPT_ANSWER: &str = "prompt_answer";
    pub const CAPTURE_MANUAL: &str = "capture_manual";
    pub const MEETINGS_LIST: &str = "meetings_list";
    pub const MEETING_GET: &str = "meeting_get";
    pub const MEETING_UPDATE: &str = "meeting_update";
    pub const MEETING_DELETE: &str = "meeting_delete";
    pub const MEETING_REPROCESS: &str = "meeting_reprocess";
    pub const SPEAKER_RENAME: &str = "speaker_rename";
    pub const ACTION_ITEM_SET_DONE: &str = "action_item_set_done";
    pub const CAPTURE_DELETE: &str = "capture_delete";
    pub const GROUPS_LIST: &str = "groups_list";
    pub const GROUP_CREATE: &str = "group_create";
    pub const GROUP_UPDATE: &str = "group_update";
    pub const GROUP_DELETE: &str = "group_delete";
    pub const GROUP_ADD_MEETING: &str = "group_add_meeting";
    pub const GROUP_REMOVE_MEETING: &str = "group_remove_meeting";
    pub const GROUP_SUGGESTIONS_LIST: &str = "group_suggestions_list";
    pub const GROUP_SUGGESTIONS_REFRESH: &str = "group_suggestions_refresh";
    pub const GROUP_SUGGESTION_ACCEPT: &str = "group_suggestion_accept";
    pub const GROUP_SUGGESTION_DISMISS: &str = "group_suggestion_dismiss";
    pub const SEARCH: &str = "search";
    pub const CHAT_THREADS_LIST: &str = "chat_threads_list";
    pub const CHAT_THREAD_CREATE: &str = "chat_thread_create";
    pub const CHAT_THREAD_DELETE: &str = "chat_thread_delete";
    pub const CHAT_MESSAGES_LIST: &str = "chat_messages_list";
    pub const CHAT_SEND: &str = "chat_send";
    pub const VOICEPRINTS_LIST: &str = "voiceprints_list";
    pub const VOICEPRINT_DELETE: &str = "voiceprint_delete";
    pub const SETTINGS_GET: &str = "settings_get";
    pub const SETTINGS_UPDATE: &str = "settings_update";
    pub const LLM_PROVIDER_TEST: &str = "llm_provider_test";
    pub const LLM_CLI_DETECT: &str = "llm_cli_detect";
    pub const SECRET_SET: &str = "secret_set";
    pub const SECRET_DELETE: &str = "secret_delete";
    pub const MODELS_STATUS: &str = "models_status";
    pub const AUDIO_INPUTS_LIST: &str = "audio_inputs_list";
    pub const MODELS_DOWNLOAD: &str = "models_download";
    pub const PERMISSIONS_STATUS: &str = "permissions_status";
    pub const PERMISSIONS_REQUEST: &str = "permissions_request";
    pub const OPEN_SYSTEM_SETTINGS: &str = "open_system_settings";
    pub const HEALTH_GET: &str = "health_get";
    pub const APP_INFO: &str = "app_info";
    pub const OVERLAY_SET_EXPANDED: &str = "overlay_set_expanded";
    pub const MAIN_WINDOW_SHOW: &str = "main_window_show";
    pub const LLM_LIST_MODELS: &str = "llm_list_models";
    pub const LLM_LIST_OPENCODE_MODELS: &str = "llm_list_opencode_models";
    pub const LLM_DETECT_LOCAL: &str = "llm_detect_local";
    pub const OPEN_EXTERNAL_URL: &str = "open_external_url";
    pub const MEETING_EXPORT_TRANSCRIPT: &str = "meeting_export_transcript";
    pub const MEETING_EXPORT_AUDIO: &str = "meeting_export_audio";
    pub const OPEN_MEETING_FOLDER: &str = "open_meeting_folder";
    pub const OPEN_DATA_FOLDER: &str = "open_data_folder";
    pub const LOCAL_LLM_STATUS: &str = "local_llm_status";
    pub const LOCAL_LLM_DOWNLOAD: &str = "local_llm_download";
    pub const LOCAL_LLM_CANCEL: &str = "local_llm_cancel";
    pub const LOCAL_LLM_DELETE: &str = "local_llm_delete";
    pub const LOCAL_LLM_TEST: &str = "local_llm_test";
    pub const MEETING_SET_NOTE: &str = "meeting_set_note";
    pub const MEETING_SET_FAVOURITE: &str = "meeting_set_favourite";
    pub const GROUP_SET_FAVOURITE: &str = "group_set_favourite";
    pub const CLEANUP_SUGGESTIONS: &str = "cleanup_suggestions";
    pub const CLEANUP_DELETE: &str = "cleanup_delete";
    pub const CLEANUP_KEEP: &str = "cleanup_keep";
    // Ola 4 (`api::live`).
    pub const MEETING_RETRY: &str = "meeting_retry";
    pub const LLM_PROVIDERS_AVAILABLE: &str = "llm_providers_available";
    pub const LIVE_TRANSCRIPT_GET: &str = "live_transcript_get";
    pub const LIVE_ASK: &str = "live_ask";
    pub const MEETING_SEARCH_TRANSCRIPT: &str = "meeting_search_transcript";
    pub const GROUPS_SUGGEST_TITLE: &str = "groups_suggest_title";
    pub const CHAT_CANCEL: &str = "chat_cancel";
    pub const LIVE_CANCEL: &str = "live_cancel";
    // Ola 5 (`api::system`).
    pub const SYSTEM_PROFILE: &str = "system_profile";
    pub const APPS_LIST: &str = "apps_list";
}

/// Estado de la sesión tal como lo ve la UI (comandos `session_*` y evento
/// `session:state`). La pregunta abierta, si la hay, está DENTRO de `state`
/// (`detected.prompt` o `recording.stopPrompt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub state: SessionState,
    /// Tiempo grabado (sin pausas) en ms.
    pub elapsed_ms: u64,
    /// Avisos de salud activos.
    pub warnings: Vec<HealthWarning>,
    /// Hay pantalla compartida y se está capturando.
    pub screen_sharing: bool,
    pub capture_count: u32,
    pub at: UnixMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendResult {
    pub user_message: ChatMessage,
    /// Id del mensaje de respuesta; llegan `chat:delta` con este id.
    pub assistant_message_id: ChatMessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliDetection {
    /// Ruta absoluta de `claude` si se encontró (login shell `zsh -lc 'command -v claude'`).
    pub claude: Option<String>,
    pub codex: Option<String>,
    #[serde(default)]
    pub opencode: Option<String>,
    /// Variables que fija el alias de `claude` (p. ej. `CLAUDE_CONFIG_DIR`),
    /// con `$HOME` ya expandido. Van a `ProviderKind::ClaudeCli.extra_env`.
    #[serde(default)]
    pub claude_env: BTreeMap<String, String>,
    #[serde(default)]
    pub codex_env: BTreeMap<String, String>,
    #[serde(default)]
    pub opencode_env: BTreeMap<String, String>,
    /// Modelo por defecto o reciente detectado en la configuración/estado de OpenCode.
    #[serde(default)]
    pub opencode_model: Option<String>,
}

/// Micrófono disponible. `uid` es lo que se guarda en
/// `Settings.recording.micDeviceUid` (`null` = el de por defecto del sistema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInput {
    pub uid: String,
    pub name: String,
    pub is_default: bool,
    /// "builtin" | "bluetooth" | "usb" | "virtual" | "other"
    pub transport: String,
}

/// Estado de un modelo local (FluidAudio: `asr`, `diarizer`, `vad`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub name: String,
    pub downloaded: bool,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub sidecar_version: Option<String>,
    pub macos_version: Option<String>,
}
