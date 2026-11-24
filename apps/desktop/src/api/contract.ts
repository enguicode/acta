// Contrato de la API que usa la UI. Implementaciones: `tauri.ts` (real) y
// `mock.ts` (tests y desarrollo en navegador). Nombres de comando/evento
// idénticos a `acta_core::api::{command_names, event_names}`.

import type * as T from "./types";

export const COMMANDS = {
  sessionGetState: "session_get_state",
  sessionStart: "session_start",
  sessionPause: "session_pause",
  sessionResume: "session_resume",
  sessionStop: "session_stop",
  sessionDismiss: "session_dismiss",
  promptAnswer: "prompt_answer",
  captureManual: "capture_manual",
  meetingsList: "meetings_list",
  meetingGet: "meeting_get",
  meetingUpdate: "meeting_update",
  meetingSetNote: "meeting_set_note",
  meetingDelete: "meeting_delete",
  meetingReprocess: "meeting_reprocess",
  speakerRename: "speaker_rename",
  actionItemSetDone: "action_item_set_done",
  captureDelete: "capture_delete",
  groupsList: "groups_list",
  groupCreate: "group_create",
  groupUpdate: "group_update",
  groupDelete: "group_delete",
  groupAddMeeting: "group_add_meeting",
  groupRemoveMeeting: "group_remove_meeting",
  groupSuggestionsList: "group_suggestions_list",
  groupSuggestionsRefresh: "group_suggestions_refresh",
  groupSuggestionAccept: "group_suggestion_accept",
  groupSuggestionDismiss: "group_suggestion_dismiss",
  search: "search",
  chatThreadsList: "chat_threads_list",
  chatThreadCreate: "chat_thread_create",
  chatThreadDelete: "chat_thread_delete",
  chatMessagesList: "chat_messages_list",
  chatSend: "chat_send",
  voiceprintsList: "voiceprints_list",
  voiceprintDelete: "voiceprint_delete",
  settingsGet: "settings_get",
  settingsUpdate: "settings_update",
  llmProviderTest: "llm_provider_test",
  llmCliDetect: "llm_cli_detect",
  secretSet: "secret_set",
  secretDelete: "secret_delete",
  modelsStatus: "models_status",
  audioInputsList: "audio_inputs_list",
  modelsDownload: "models_download",
  permissionsStatus: "permissions_status",
  permissionsRequest: "permissions_request",
  openSystemSettings: "open_system_settings",
  healthGet: "health_get",
  appInfo: "app_info",
  overlaySetExpanded: "overlay_set_expanded",
  mainWindowShow: "main_window_show",
  // src-tauri/src/commands/providers_ui.rs
  llmListModels: "llm_list_models",
  llmListOpencodeModels: "llm_list_opencode_models",
  llmDetectLocal: "llm_detect_local",
  openExternalUrl: "open_external_url",
  // src-tauri/src/commands/downloads.rs
  meetingExportTranscript: "meeting_export_transcript",
  meetingExportAudio: "meeting_export_audio",
  openMeetingFolder: "open_meeting_folder",
  openDataFolder: "open_data_folder",
  meetingSetFavourite: "meeting_set_favourite",
  groupSetFavourite: "group_set_favourite",
  cleanupSuggestions: "cleanup_suggestions",
  cleanupDelete: "cleanup_delete",
  cleanupKeep: "cleanup_keep",
  localLlmStatus: "local_llm_status",
  localLlmDownload: "local_llm_download",
  localLlmCancel: "local_llm_cancel",
  localLlmDelete: "local_llm_delete",
  localLlmTest: "local_llm_test",
  // ola 4 (acta_core::api::live)
  meetingRetry: "meeting_retry",
  llmProvidersAvailable: "llm_providers_available",
  liveTranscriptGet: "live_transcript_get",
  liveAsk: "live_ask",
  meetingSearchTranscript: "meeting_search_transcript",
  groupsSuggestTitle: "groups_suggest_title",
  chatCancel: "chat_cancel",
  liveCancel: "live_cancel",
  // ola 5 (acta_core::api::system)
  systemProfile: "system_profile",
  appsList: "apps_list",
} as const;

export const EVENTS = {
  sessionState: "session:state",
  sessionLevels: "session:levels",
  sessionHealth: "session:health",
  processingProgress: "processing:progress",
  meetingUpdated: "meeting:updated",
  captureSaved: "capture:saved",
  chatDelta: "chat:delta",
  livePartial: "live:partial",
  modelDownload: "model:download",
  localLlmDownload: "localmodel:download",
  mainNavigate: "main:navigate",
  liveTranscript: "live:transcript",
  liveAnswer: "live:answer",
  permissionsChanged: "permissions:changed",
} as const;

/** Payload de cada evento. */
export interface EventPayloads {
  "session:state": T.SessionSnapshot;
  "session:levels": T.Levels;
  "session:health": T.HealthWarning;
  "processing:progress": T.ProcessingProgress;
  "meeting:updated": T.MeetingUpdated;
  "capture:saved": T.Capture;
  "chat:delta": T.ChatDelta;
  "live:partial": T.LivePartial;
  "model:download": T.ModelDownload;
  "localmodel:download": T.LocalLlmDownload;
  "main:navigate": T.MainNavigate;
  "live:transcript": T.LiveTranscript;
  "live:answer": T.LiveAnswer;
  "permissions:changed": T.PermissionChanged;
}

export type EventName = keyof EventPayloads;
export type Unsubscribe = () => void;

export interface ActaApi {
  // sesión
  sessionGetState(): Promise<T.SessionSnapshot>;
  sessionStart(app?: T.MeetingApp | null): Promise<T.SessionSnapshot>;
  sessionPause(): Promise<T.SessionSnapshot>;
  sessionResume(): Promise<T.SessionSnapshot>;
  sessionStop(): Promise<T.SessionSnapshot>;
  sessionDismiss(): Promise<T.SessionSnapshot>;
  promptAnswer(promptId: T.PromptId, answer: T.PromptAnswer): Promise<T.SessionSnapshot>;
  captureManual(): Promise<T.Capture | null>;

  // reuniones
  meetingsList(query: T.MeetingQuery): Promise<T.Page<T.MeetingSummary>>;
  meetingGet(id: T.MeetingId): Promise<T.MeetingDetail>;
  meetingUpdate(id: T.MeetingId, title: string): Promise<T.Meeting>;
  /** Nota personal de la reunión; en blanco la borra. */
  meetingSetNote(id: T.MeetingId, note: string): Promise<T.Meeting>;
  meetingDelete(id: T.MeetingId): Promise<void>;
  /** Pone o quita la estrella. Las favoritas nunca se sugieren para borrar. */
  meetingSetFavourite(id: T.MeetingId, favourite: boolean): Promise<T.Meeting>;
  meetingReprocess(request: T.ReprocessRequest): Promise<void>;
  /** «Reintentar» (opcionalmente «con…» otro proveedor/modelo solo esta vez). Sin `steps`: desde transcribir. */
  meetingRetry(request: T.MeetingRetryRequest): Promise<void>;
  /** Coincidencias dentro del transcript, sin distinguir acentos ni mayúsculas. */
  meetingSearchTranscript(meetingId: T.MeetingId, query: string): Promise<T.TranscriptMatch[]>;
  speakerRename(
    meetingId: T.MeetingId,
    speakerId: T.SpeakerId,
    displayName: string,
    remember: boolean,
  ): Promise<void>;
  actionItemSetDone(id: T.ActionItemId, done: boolean): Promise<void>;
  captureDelete(id: T.CaptureId): Promise<void>;

  // grupos
  groupsList(): Promise<T.Group[]>;
  groupCreate(name: string, description?: string | null): Promise<T.Group>;
  groupUpdate(id: T.GroupId, name?: string | null, description?: string | null): Promise<T.Group>;
  groupDelete(id: T.GroupId): Promise<void>;
  groupSetFavourite(id: T.GroupId, favourite: boolean): Promise<T.Group>;
  groupAddMeeting(groupId: T.GroupId, meetingId: T.MeetingId): Promise<void>;
  groupRemoveMeeting(groupId: T.GroupId, meetingId: T.MeetingId): Promise<void>;
  groupSuggestionsList(): Promise<T.GroupSuggestion[]>;
  groupSuggestionsRefresh(): Promise<T.GroupSuggestion[]>;
  groupSuggestionAccept(id: T.GroupId, name?: string | null): Promise<T.Group>;
  groupSuggestionDismiss(id: T.GroupId): Promise<void>;
  /** Nombre sacado de lo hablado en sus reuniones. No lo aplica: usa `groupUpdate`. */
  groupsSuggestTitle(groupId: T.GroupId): Promise<T.GroupTitleSuggestion>;

  // en vivo (mientras graba)
  /** Todo lo transcrito hasta ahora en la grabación `sessionId`. */
  liveTranscriptGet(sessionId: T.MeetingId): Promise<T.LiveTranscript>;
  /** Pregunta sobre lo dicho hasta ahora. Devuelve el `requestId`; la respuesta llega por `live:answer`. */
  liveAsk(request: T.LiveAskRequest): Promise<string>;
  /** «Detener» de una pregunta en vivo. Llega un `live:answer` con `done: true`. */
  liveCancel(requestId: string): Promise<void>;

  // limpieza de reuniones antiguas (solo sugiere)
  /** Vacío si las sugerencias están apagadas en Ajustes. */
  cleanupSuggestions(): Promise<T.CleanupSuggestions>;
  /** Borra audio, capturas, transcript e índice de las elegidas. Nunca favoritas. */
  cleanupDelete(ids: T.MeetingId[]): Promise<T.CleanupResult>;
  /** «Mantener»: esas reuniones no se vuelven a sugerir. */
  cleanupKeep(ids: T.MeetingId[]): Promise<void>;

  // búsqueda y chat
  search(query: T.SearchQuery): Promise<T.SearchHit[]>;
  chatThreadsList(scope?: T.ChatScope | null): Promise<T.ChatThread[]>;
  chatThreadCreate(scope: T.ChatScope): Promise<T.ChatThread>;
  chatThreadDelete(threadId: T.ChatThreadId): Promise<void>;
  chatMessagesList(threadId: T.ChatThreadId): Promise<T.ChatMessage[]>;
  /** `llm`: proveedor y modelo SOLO para esta respuesta (no cambia los ajustes). Sin él, el proveedor del chat. */
  chatSend(threadId: T.ChatThreadId, text: string, llm?: T.LlmOverride | null): Promise<T.ChatSendResult>;
  /** «Detener»: corta la generación de `chatSend` (`requestId` = `assistantMessageId`). Lo recibido se guarda. */
  chatCancel(requestId: T.ChatMessageId): Promise<void>;

  // huellas de voz
  voiceprintsList(): Promise<T.VoiceprintSummary[]>;
  voiceprintDelete(id: T.VoiceprintId): Promise<void>;

  // ajustes, permisos, diagnóstico
  settingsGet(): Promise<T.Settings>;
  settingsUpdate(settings: T.Settings): Promise<T.Settings>;
  llmProviderTest(providerId: T.ProviderId): Promise<T.ProviderTestResult>;
  llmCliDetect(): Promise<T.CliDetection>;
  /** Modelos de texto de un servidor compatible con OpenAI. Con `providerId`, usa su clave del Llavero. */
  llmListModels(baseUrl: string, providerId?: T.ProviderId | null): Promise<string[]>;
  /** Modelos disponibles en el CLI de OpenCode (`opencode models`). */
  llmListOpencodeModels(binaryPath: string, providerId?: T.ProviderId | null): Promise<string[]>;
  /** Ollama y LM Studio en sus puertos por defecto. Que no estén no es un error. */
  llmDetectLocal(): Promise<T.LocalDetection>;
  /** Proveedores activados con sus modelos elegibles (menú «Reintentar con…»). */
  llmProvidersAvailable(): Promise<T.AvailableProvider[]>;
  /** Modelo local integrado: hardware, niveles, qué hay descargado y recomendación. */
  localLlmStatus(): Promise<T.LocalLlmStatus>;
  /** Descarga (reanudable) llama.cpp y el modelo. Progreso: `localmodel:download`. */
  localLlmDownload(modelId: string): Promise<void>;
  /** Pausa la descarga; lo bajado queda para reanudar. */
  localLlmCancel(modelId: string): Promise<void>;
  localLlmDelete(modelId: string): Promise<void>;
  /** Carga el modelo y le pide una palabra. */
  localLlmTest(modelId: string): Promise<T.ProviderTestResult>;
  secretSet(providerId: T.ProviderId, value: string): Promise<T.SecretRef>;
  secretDelete(providerId: T.ProviderId): Promise<void>;
  modelsStatus(): Promise<T.ModelStatus[]>;
  /** Micrófonos del sistema, para elegir `settings.recording.micDeviceUid`. */
  audioInputsList(): Promise<T.AudioInput[]>;
  modelsDownload(name: string): Promise<void>;
  permissionsStatus(): Promise<T.PermissionState[]>;
  permissionsRequest(kind: T.PermissionKind): Promise<T.PermissionStatus>;
  openSystemSettings(kind: T.PermissionKind): Promise<void>;
  healthGet(): Promise<T.HealthWarning[]>;
  appInfo(): Promise<T.AppInfo>;
  /** Nombre de la cuenta e idiomas de macOS, para proponerlos en la bienvenida. */
  systemProfile(): Promise<T.SystemProfile>;
  /** Apps instaladas (/Applications, ~/Applications) y en marcha, con icono. Ordenadas por nombre. */
  appsList(): Promise<T.InstalledApp[]>;
  /** Abre un enlace web en el navegador por defecto (nunca dentro de acta). */
  openExternalUrl(url: string): Promise<void>;
  /** Guarda la transcripción (nombres y tiempos) en Descargas y la muestra en el Finder. */
  meetingExportTranscript(meetingId: T.MeetingId, format: T.TranscriptFormat): Promise<T.ExportedFile>;
  /** Copia el audio final (la mezcla) de la reunión a Descargas y lo muestra en el Finder. */
  meetingExportAudio(meetingId: T.MeetingId): Promise<T.ExportedFile>;
  /** Abre en el Finder la carpeta de la reunión (audio, capturas, transcript). */
  openMeetingFolder(meetingId: T.MeetingId): Promise<void>;
  /** Abre en el Finder la carpeta de datos de acta (`AppInfo.dataDir`). */
  openDataFolder(): Promise<void>;

  // ventanas
  overlaySetExpanded(expanded: boolean): Promise<void>;
  mainWindowShow(meetingId?: T.MeetingId | null): Promise<void>;

  /** Suscripción tipada a un evento del backend. */
  on<E extends EventName>(event: E, handler: (payload: EventPayloads[E]) => void): Promise<Unsubscribe>;

  /** URL reproducible/visualizable para una ruta absoluta local (audio, capturas). */
  fileUrl(absolutePath: string): string;
}
