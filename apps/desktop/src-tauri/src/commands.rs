//! Comandos Tauri (contrato: `acta_core::api::commands`). Son envoltorios
//! finos de `AppCore`; la lógica y sus tests viven en `app.rs`.

use crate::app::AppCore;
use crate::overlay::{EXPANDED_SIZE, PILL_SIZE};
use acta_core::api::*;
use acta_core::chat::{ChatMessage, ChatScope, ChatThread};
use acta_core::detection::MeetingApp;
use acta_core::ids::*;
use acta_core::library::*;
use acta_core::llm::{ProviderTestResult, SecretRef};
use acta_core::permissions::{PermissionKind, PermissionState, PermissionStatus};
use acta_core::processing::ReprocessRequest;
use acta_core::recording::{HealthWarning, PromptAnswer};
use acta_core::screen::Capture;
use acta_core::settings::Settings;
use acta_core::transcription::VoiceprintSummary;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

pub mod downloads;
pub mod providers_ui;

pub type Core = Arc<AppCore>;

macro_rules! cmd {
    ($name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty => |$core:ident| $body:expr) => {
        #[tauri::command]
        pub async fn $name(state: State<'_, Core>, $($arg: $ty),*) -> ApiResult<$ret> {
            let $core: Core = state.inner().clone();
            $body.await.map_err(ApiError::from)
        }
    };
}

cmd!(session_get_state() -> SessionSnapshot => |c| c.session_get_state());
cmd!(session_start(app: Option<MeetingApp>) -> SessionSnapshot => |c| c.session_start(app));
cmd!(session_pause() -> SessionSnapshot => |c| c.session_pause());
cmd!(session_resume() -> SessionSnapshot => |c| c.session_resume());
cmd!(session_stop() -> SessionSnapshot => |c| c.session_stop());
cmd!(session_dismiss() -> SessionSnapshot => |c| c.session_dismiss());
cmd!(prompt_answer(prompt_id: PromptId, answer: PromptAnswer) -> SessionSnapshot => |c| c.prompt_answer(prompt_id, answer));
cmd!(capture_manual() -> Option<Capture> => |c| c.capture_manual());
cmd!(meetings_list(query: MeetingQuery) -> Page<MeetingSummary> => |c| c.meetings_list(query));
cmd!(meeting_get(id: MeetingId) -> MeetingDetail => |c| c.meeting_get(id));
cmd!(meeting_update(id: MeetingId, title: String) -> Meeting => |c| c.meeting_update(id, title));
cmd!(meeting_set_note(id: MeetingId, note: String) -> Meeting => |c| c.meeting_set_note(id, note));
cmd!(meeting_delete(id: MeetingId) -> () => |c| c.meeting_delete(id));
cmd!(meeting_set_favourite(id: MeetingId, favourite: bool) -> Meeting => |c| c.meeting_set_favourite(id, favourite));
cmd!(group_set_favourite(id: GroupId, favourite: bool) -> Group => |c| c.group_set_favourite(id, favourite));
cmd!(cleanup_suggestions() -> CleanupSuggestions => |c| c.cleanup_suggestions());
cmd!(cleanup_delete(ids: Vec<MeetingId>) -> CleanupResult => |c| c.cleanup_delete(ids));
cmd!(cleanup_keep(ids: Vec<MeetingId>) -> () => |c| c.cleanup_keep(ids));
cmd!(meeting_reprocess(request: ReprocessRequest) -> () => |c| c.meeting_reprocess(request));
cmd!(meeting_retry(meeting_id: MeetingId, steps: Option<Vec<acta_core::processing::ProcessingStep>>, provider_id: Option<ProviderId>, model: Option<String>) -> () =>
    |c| c.meeting_retry(meeting_id, steps, provider_id, model));
cmd!(meeting_search_transcript(meeting_id: MeetingId, query: String) -> Vec<TranscriptMatch> => |c| c.meeting_search_transcript(meeting_id, query));
cmd!(groups_suggest_title(group_id: GroupId) -> GroupTitleSuggestion => |c| c.groups_suggest_title(group_id));
cmd!(llm_providers_available() -> Vec<AvailableProvider> => |c| c.llm_providers_available());
cmd!(chat_cancel(request_id: String) -> () => |c| c.chat_cancel(request_id));
cmd!(live_cancel(request_id: String) -> () => |c| c.chat_cancel(request_id));
cmd!(live_transcript_get(session_id: MeetingId) -> LiveTranscript => |c| c.live_transcript_get(session_id));
cmd!(live_ask(session_id: MeetingId, question: String, provider_id: Option<ProviderId>, model: Option<String>) -> String =>
    |c| c.live_ask(session_id, question, provider_id, model));
cmd!(speaker_rename(meeting_id: MeetingId, speaker_id: SpeakerId, display_name: String, remember: bool) -> () =>
    |c| c.speaker_rename(meeting_id, speaker_id, display_name, remember));
cmd!(action_item_set_done(id: ActionItemId, done: bool) -> () => |c| c.action_item_set_done(id, done));
cmd!(capture_delete(id: CaptureId) -> () => |c| c.capture_delete(id));
cmd!(groups_list() -> Vec<Group> => |c| c.groups_list());
cmd!(group_create(name: String, description: Option<String>) -> Group => |c| c.group_create(name, description));
cmd!(group_update(id: GroupId, name: Option<String>, description: Option<String>) -> Group => |c| c.group_update(id, name, description));
cmd!(group_delete(id: GroupId) -> () => |c| c.group_delete(id));
cmd!(group_add_meeting(group_id: GroupId, meeting_id: MeetingId) -> () => |c| c.group_add_meeting(group_id, meeting_id));
cmd!(group_remove_meeting(group_id: GroupId, meeting_id: MeetingId) -> () => |c| c.group_remove_meeting(group_id, meeting_id));
cmd!(group_suggestions_list() -> Vec<GroupSuggestion> => |c| c.group_suggestions_list());
cmd!(group_suggestions_refresh() -> Vec<GroupSuggestion> => |c| c.group_suggestions_refresh());
cmd!(group_suggestion_accept(id: GroupId, name: Option<String>) -> Group => |c| c.group_suggestion_accept(id, name));
cmd!(group_suggestion_dismiss(id: GroupId) -> () => |c| c.group_suggestion_dismiss(id));
cmd!(search(query: SearchQuery) -> Vec<SearchHit> => |c| c.search(query));
cmd!(chat_threads_list(scope: Option<ChatScope>) -> Vec<ChatThread> => |c| c.chat_threads_list(scope));
cmd!(chat_thread_create(scope: ChatScope) -> ChatThread => |c| c.chat_thread_create(scope));
cmd!(chat_thread_delete(thread_id: ChatThreadId) -> () => |c| c.chat_thread_delete(thread_id));
cmd!(chat_messages_list(thread_id: ChatThreadId) -> Vec<ChatMessage> => |c| c.chat_messages_list(thread_id));
cmd!(chat_send(thread_id: ChatThreadId, text: String, provider_id: Option<ProviderId>, model: Option<String>) -> ChatSendResult =>
    |c| c.chat_send(thread_id, text, provider_id, model));
cmd!(system_profile() -> SystemProfile => |c| c.system_profile());
cmd!(apps_list() -> Vec<InstalledApp> => |c| c.apps_list());
cmd!(voiceprints_list() -> Vec<VoiceprintSummary> => |c| c.voiceprints_list());
cmd!(voiceprint_delete(id: VoiceprintId) -> () => |c| c.voiceprint_delete(id));
cmd!(settings_get() -> Settings => |c| c.settings_get());
cmd!(settings_update(settings: Settings) -> Settings => |c| c.settings_update(settings));
cmd!(llm_provider_test(provider_id: ProviderId) -> ProviderTestResult => |c| c.llm_provider_test(provider_id));
cmd!(llm_cli_detect() -> CliDetection => |c| c.llm_cli_detect());
cmd!(secret_set(provider_id: ProviderId, value: String) -> SecretRef => |c| c.secret_set(provider_id, value));
cmd!(secret_delete(provider_id: ProviderId) -> () => |c| c.secret_delete(provider_id));
cmd!(local_llm_status() -> acta_core::llm::local::LocalLlmStatus => |c| c.local_llm_status());
cmd!(local_llm_download(model_id: String) -> () => |c| c.local_llm_download(model_id));
cmd!(local_llm_cancel(model_id: String) -> () => |c| c.local_llm_cancel(model_id));
cmd!(local_llm_delete(model_id: String) -> () => |c| c.local_llm_delete(model_id));
cmd!(local_llm_test(model_id: String) -> ProviderTestResult => |c| c.local_llm_test(model_id));
cmd!(models_status() -> Vec<ModelStatus> => |c| c.models_status());
cmd!(audio_inputs_list() -> Vec<AudioInput> => |c| c.audio_inputs_list());
cmd!(models_download(name: String) -> () => |c| c.models_download(name));
cmd!(permissions_status() -> Vec<PermissionState> => |c| c.permissions_status());
cmd!(permissions_request(kind: PermissionKind) -> PermissionStatus => |c| c.permissions_request(kind));
cmd!(health_get() -> Vec<HealthWarning> => |c| c.health_get());

#[tauri::command]
pub async fn app_info(state: State<'_, Core>) -> ApiResult<AppInfo> {
    Ok(state.app_info())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError { code: ApiErrorCode::Internal, message: e.to_string() }
}

#[tauri::command]
pub async fn open_system_settings(app: AppHandle, kind: PermissionKind) -> ApiResult<()> {
    app.opener()
        .open_url(crate::system::settings_url(kind), None::<&str>)
        .map_err(internal)
}

#[tauri::command]
pub async fn overlay_set_expanded(app: AppHandle, expanded: bool) -> ApiResult<()> {
    let Some(win) = app.get_webview_window("overlay") else { return Ok(()) };
    let (w, h) = if expanded { EXPANDED_SIZE } else { PILL_SIZE };
    // Crece hacia los lados y, abajo en la pantalla, hacia arriba.
    let scale = win.scale_factor().map_err(internal)?;
    let pos = win.outer_position().map_err(internal)?.to_logical::<f64>(scale);
    let size = win.outer_size().map_err(internal)?.to_logical::<f64>(scale);
    let screen = match win.current_monitor().map_err(internal)? {
        Some(m) => crate::overlay::monitor_rects(&m).1,
        None => crate::overlay::Rect { x: 0.0, y: 0.0, w: 1e5, h: 1e5 },
    };
    let (x, y) = crate::overlay::anchored_origin((pos.x, pos.y), (size.width, size.height), (w, h), screen);
    win.set_size(tauri::LogicalSize::new(w, h)).map_err(internal)?;
    win.set_position(tauri::LogicalPosition::new(x, y)).map_err(internal)
}

/// Muestra la ventana principal y, si se pide, abre esa reunión.
pub fn show_main(app: &AppHandle, meeting_id: Option<MeetingId>) -> tauri::Result<()> {
    if let Some(win) = app.get_webview_window("main") {
        win.show()?;
        win.unminimize()?;
        win.set_focus()?;
    }
    if let Some(meeting_id) = meeting_id {
        app.emit_to("main", event_names::MAIN_NAVIGATE, MainNavigate { meeting_id })?;
    }
    Ok(())
}

#[tauri::command]
pub async fn main_window_show(app: AppHandle, meeting_id: Option<MeetingId>) -> ApiResult<()> {
    show_main(&app, meeting_id).map_err(internal)
}
