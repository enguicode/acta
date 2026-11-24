// Espejo TypeScript de los DTO de `crates/acta-core` (contrato Tauri).
// Si cambias algo aquí, cambia el tipo Rust en el MISMO commit (y viceversa).
//
// - Ids: string (UUID v7).
// - UnixMs: ms desde epoch (reloj de pared). MediaMs: ms en el audio de la reunión (sin pausas).
// - Todo en camelCase salvo `Escaleta` (claves en castellano snake_case, igual que el JSON del LLM).

export type Id = string;
export type MeetingId = Id;
export type SpeakerId = Id;
export type ActionItemId = Id;
export type DecisionId = Id;
export type CaptureId = Id;
export type GroupId = Id;
export type VoiceprintId = Id;
export type ChatThreadId = Id;
export type ChatMessageId = Id;
export type PromptId = Id;
export type ProviderId = string;
export type UnixMs = number;
export type MediaMs = number;

// ---------------------------------------------------------------- detección

export type Platform = "teams" | "meet" | "zoom" | "slack" | "other";

export interface MeetingApp {
  platform: Platform;
  bundleId: string;
  pid: number;
  audioPids: number[];
  name: string;
  title: string | null;
  url: string | null;
}

export interface AppRule {
  bundleId: string;
  platform: Platform;
  isBrowser: boolean;
}

// ---------------------------------------------------------------- grabación

export type Track = "mic" | "system";

export type StopReason =
  | "user"
  | "meetingEndedConfirmed"
  | "autoSilence"
  | "maxDuration"
  | "appQuit"
  | "error"
  | "discarded";

export type StopPromptReason = "meetingEnded" | "silence" | "autoStarted";

export type PromptKind =
  | { kind: "startRecording"; app: MeetingApp }
  | { kind: "stopRecording"; reason: StopPromptReason };

/** `kind` va aplanado dentro del prompt. */
export type Prompt = PromptKind & { id: PromptId; createdAt: UnixMs };

export type PromptAnswer = "accept" | "decline";

export type ProcessingStep =
  | "concatenating"
  | "transcribing"
  | "diarizing"
  | "namingSpeakers"
  | "summarizing"
  | "indexing"
  | "pruningCaptures";

export interface ActiveRecording {
  meetingId: MeetingId;
  app: MeetingApp | null;
  startedAt: UnixMs;
  stopPrompt: Prompt | null;
}

export type SessionState =
  | { state: "idle" }
  | { state: "detected"; prompt: Prompt; app: MeetingApp }
  | ({ state: "recording" } & ActiveRecording)
  | ({ state: "paused"; pausedAt: UnixMs } & ActiveRecording)
  | { state: "stopping"; meetingId: MeetingId; reason: StopReason }
  | { state: "processing"; meetingId: MeetingId; step: ProcessingStep }
  | { state: "ready"; meetingId: MeetingId }
  | { state: "recovering"; meetingId: MeetingId }
  | { state: "failed"; meetingId: MeetingId | null; error: string };

export interface Level {
  rmsDbfs: number;
  peakDbfs: number;
  /** Silencio digital exacto: firma de permiso TCC denegado. */
  allZero: boolean;
}

export interface Levels {
  mic: Level;
  system: Level;
  at: UnixMs;
}

export type PermissionKind =
  | "microphone"
  | "systemAudio"
  | "screenRecording"
  | "accessibility"
  | "automation";

export type PermissionStatus = "granted" | "denied" | "notDetermined" | "unknown";

export interface PermissionState {
  kind: PermissionKind;
  status: PermissionStatus;
  target: string | null;
}

export type HealthIssue =
  | { kind: "trackStalled"; track: Track }
  | { kind: "trackAllZeros"; track: Track }
  | { kind: "segmentsStalled"; track: Track }
  | { kind: "diskLow"; freeBytes: number }
  | { kind: "overrun"; track: Track; droppedFrames: number }
  | { kind: "sidecarRestarted"; count: number }
  | { kind: "writeFailed"; track: Track; message: string }
  | { kind: "permissionMissing"; permission: PermissionKind };

export interface HealthWarning {
  issue: HealthIssue;
  since: UnixMs;
  cleared: boolean;
  /** En castellano, apto para mostrar. */
  message: string;
}

export interface SessionSnapshot {
  state: SessionState;
  elapsedMs: number;
  warnings: HealthWarning[];
  screenSharing: boolean;
  captureCount: number;
  at: UnixMs;
}

export interface ProcessingProgress {
  meetingId: MeetingId;
  step: ProcessingStep;
  fraction: number;
}

export interface ReprocessRequest {
  meetingId: MeetingId;
  steps: ProcessingStep[];
  /** Proveedor/modelo solo para este reintento (no cambia los ajustes). */
  llm?: LlmOverride | null;
}

/** `meeting_retry`. Sin `steps`: desde transcribir. Proveedor/modelo solo para esta vez. */
export interface MeetingRetryRequest {
  meetingId: MeetingId;
  steps?: ProcessingStep[] | null;
  providerId?: ProviderId | null;
  model?: string | null;
}

/** `live_ask`. Sin proveedor: el del chat. */
export interface LiveAskRequest {
  sessionId: MeetingId;
  question: string;
  providerId?: ProviderId | null;
  model?: string | null;
}

/** `permissions:changed`: un permiso cambió (se comprueba al volver a la app). */
export interface PermissionChanged {
  kind: PermissionKind;
  status: PermissionStatus;
}

/** «Reintentar con…»: proveedor y, opcionalmente, modelo. */
export interface LlmOverride {
  providerId: ProviderId;
  model?: string | null;
}

// ---------------------------------------------------------------- ola 4: en vivo y búsqueda
// Espejo de `acta_core::api::live`.

/** `"yo"` (micro), `"otros"` (sistema sin diarizar) o un nombre (Teams). */
export type LiveSpeaker = "yo" | "otros" | (string & {});

export interface LiveSegment {
  /** Estable: el provisional y su versión final comparten id. */
  id: string;
  speaker: LiveSpeaker;
  text: string;
  /** Tiempo media desde el inicio de la grabación. */
  startMs: number;
  endMs: number;
  /** `false` mientras la ventana de audio aún puede cambiar. */
  final: boolean;
}

/** `live:transcript` trae los segmentos NUEVOS o CAMBIADOS (fusionar por `id`); `liveTranscriptGet` trae todos. */
export interface LiveTranscript {
  /** `MeetingId` de la grabación en curso. */
  sessionId: MeetingId;
  segments: LiveSegment[];
}

export interface LiveCitation {
  startMs: number;
}

/** `live:answer`: streaming de `liveAsk`. El último lleva `done: true`. */
export interface LiveAnswer {
  requestId: string;
  delta?: string;
  done: boolean;
  /** Solo con `done: true`: momentos citados. */
  citations?: LiveCitation[];
  /** En castellano, apto para mostrar. Llega con `done: true`. */
  error?: string;
}

/** Proveedor activado y sus modelos elegibles («Reintentar con…»). */
export interface AvailableProvider {
  providerId: ProviderId;
  label: string;
  kind: ProviderKind["type"];
  /** Puede estar vacío (el CLI usa su modelo por defecto). */
  models: string[];
  defaultModel: string | null;
  /** Es el proveedor por defecto del acta (`minutesProvider`). */
  isDefault: boolean;
  /** Para qué es el proveedor por defecto: "minutes" | "chat" | "naming". */
  defaultFor: ("minutes" | "chat" | "naming")[];
}

export interface TranscriptMatch {
  /** `Utterance.idx`. */
  utteranceId: number;
  startMs: number;
  /** Texto entero de la utterance. */
  snippet: string;
  /** `[inicio, fin)` en unidades UTF-16 de `snippet` (vale con `String.slice`). */
  ranges: [number, number][];
}

export interface GroupTitleSuggestion {
  name: string;
  description: string | null;
}

// ---------------------------------------------------------------- transcript

/** `transcript` = deducido de lo dicho en la reunión ("Hola, soy Marta"). */
export type NameSource = "me" | "user" | "appUi" | "voiceprint" | "transcript" | "generic";

export interface Speaker {
  id: SpeakerId;
  meetingId: MeetingId;
  label: string;
  displayName: string | null;
  isMe: boolean;
  track: Track;
  diarizationLabel: string | null;
  nameSource: NameSource;
  confirmed: boolean;
  voiceprintId: VoiceprintId | null;
}

export interface Utterance {
  idx: number;
  speakerId: SpeakerId;
  track: Track;
  start: MediaMs;
  end: MediaMs;
  text: string;
  confidence: number | null;
}

export interface Transcript {
  meetingId: MeetingId;
  speakers: Speaker[];
  utterances: Utterance[];
}

export interface VoiceprintSummary {
  id: VoiceprintId;
  displayName: string;
  samples: number;
  meetings: number;
  updatedAt: UnixMs;
}

export interface LivePartial {
  track: Track;
  text: string;
  at: UnixMs;
  isFinal: boolean;
}

// ---------------------------------------------------------------- acta

export interface Decision {
  id: DecisionId;
  text: string;
  at: MediaMs | null;
}

export interface ActionItem {
  id: ActionItemId;
  meetingId: MeetingId;
  text: string;
  ownerName: string | null;
  ownerSpeakerId: SpeakerId | null;
  /** "Mis tareas" (en tu tejado). */
  isMine: boolean;
  due: string | null;
  done: boolean;
  at: MediaMs | null;
  captureIds: CaptureId[];
}

export interface Minutes {
  meetingId: MeetingId;
  summaryMd: string;
  decisions: Decision[];
  actionItems: ActionItem[];
  provider: ProviderId;
  model: string | null;
  generatedAt: UnixMs;
}

/** EXCEPCIÓN de nombres: claves en castellano snake_case. */
export interface Tema {
  titulo: string;
  resumen: string;
  inicio_ms: MediaMs;
  fin_ms: MediaMs;
  subtemas: Tema[];
  /** CaptureId[] una vez guardada. */
  capturas: string[];
}

export interface Escaleta {
  temas: Tema[];
}

export type TimelineKind = "topic" | "decision" | "actionItem" | "capture" | "screenShare";

export interface TimelineEvent {
  at: MediaMs;
  end: MediaMs | null;
  kind: TimelineKind;
  label: string;
  refId: string | null;
}

// ---------------------------------------------------------------- capturas

export type CaptureReason = "slideChange" | "keyword" | "llmPick" | "manual" | "candidate";

export interface Capture {
  id: CaptureId;
  meetingId: MeetingId;
  at: MediaMs;
  atWall: UnixMs;
  /** Relativa a la carpeta de la reunión. */
  path: string;
  reason: CaptureReason;
  phash: string;
  width: number;
  height: number;
  keyword: string | null;
  topicPath: string | null;
  actionItemIds: ActionItemId[];
}

// ---------------------------------------------------------------- biblioteca

export type MeetingStatus = "recording" | "processing" | "ready" | "failed";

export interface Meeting {
  id: MeetingId;
  title: string;
  platform: Platform;
  appBundleId: string | null;
  startedAt: UnixMs;
  endedAt: UnixMs | null;
  duration: MediaMs;
  status: MeetingStatus;
  recovered: boolean;
  error: string | null;
  language: string;
  createdAt: UnixMs;
  updatedAt: UnixMs;
  /** Marcada con la estrella. */
  favourite: boolean;
  /** Si falló el resumen: proveedor y modelo que fallaron (el texto de `error` también los nombra). */
  errorProvider?: string | null;
  errorModel?: string | null;
  /** Nota personal del usuario («nota para mí»): privada, va al LLM como contexto y sale en el acta en su sección. */
  userNote?: string | null;
}

export interface MeetingSummary {
  id: MeetingId;
  title: string;
  platform: Platform;
  startedAt: UnixMs;
  duration: MediaMs;
  status: MeetingStatus;
  speakerNames: string[];
  myOpenTasks: number;
  groupIds: GroupId[];
  captureCount: number;
  snippet: string | null;
  /** Estrella de la reunión (no cuenta la de sus grupos). */
  favourite: boolean;
}

/** Rutas absolutas; usar `convertFileSrc` para reproducir. */
export interface MeetingAudio {
  mic: string | null;
  system: string | null;
  mix: string | null;
}

export interface MeetingDetail {
  meeting: Meeting;
  transcript: Transcript;
  minutes: Minutes | null;
  escaleta: Escaleta | null;
  captures: Capture[];
  timeline: TimelineEvent[];
  groups: Group[];
  audio: MeetingAudio;
}

export interface MeetingQuery {
  text?: string | null;
  groupId?: GroupId | null;
  platform?: Platform | null;
  from?: UnixMs | null;
  to?: UnixMs | null;
  /** Solo favoritas: reuniones con estrella y reuniones de grupos con estrella. */
  favouritesOnly?: boolean;
  limit?: number | null;
  cursor?: string | null;
}

export interface Page<T> {
  items: T[];
  nextCursor: string | null;
}

export type GroupKind = "manual" | "suggested";

export interface Group {
  id: GroupId;
  name: string;
  description: string | null;
  kind: GroupKind;
  meetingCount: number;
  createdAt: UnixMs;
  updatedAt: UnixMs;
  /** Grupo con estrella: sus reuniones cuentan como favoritas. */
  favourite: boolean;
}

// ---------------------------------------------------------------- limpieza

/** Por qué se sugiere borrar una reunión (espejo de `library::cleanup::CleanupReason`). */
export type CleanupReason =
  | { kind: "old"; days: number }
  | { kind: "neverUsed" }
  | { kind: "unusedFor"; days: number }
  | { kind: "short"; durationMs: number }
  | { kind: "littleContent"; words: number }
  | { kind: "failed" };

export interface CleanupCandidate {
  meetingId: MeetingId;
  title: string;
  startedAt: UnixMs;
  duration: MediaMs;
  reasons: CleanupReason[];
  /** Lo que ocupa en disco (audio, capturas, transcript). */
  bytes: number;
}

export interface CleanupSuggestions {
  candidates: CleanupCandidate[];
  totalBytes: number;
  minAgeDays: number;
}

export interface CleanupResult {
  deleted: MeetingId[];
  /** No se borraron: favoritas, en grabación o ya inexistentes. */
  skipped: MeetingId[];
  freedBytes: number;
}

export interface GroupSuggestion {
  id: GroupId;
  name: string | null;
  description: string | null;
  meetingIds: MeetingId[];
  cohesion: number;
  createdAt: UnixMs;
}

export type SearchScope =
  | { kind: "all" }
  /** Reuniones con estrella y reuniones de grupos con estrella. */
  | { kind: "favourites" }
  | { kind: "group"; groupId: GroupId }
  | { kind: "meeting"; meetingId: MeetingId };

export interface SearchQuery {
  text: string;
  scope: SearchScope;
  limit: number;
}

export interface SearchHit {
  meetingId: MeetingId;
  meetingTitle: string;
  startedAt: UnixMs;
  at: MediaMs;
  end: MediaMs;
  speaker: string | null;
  snippet: string;
  score: number;
}

// ---------------------------------------------------------------- chat

export type ChatScope =
  | { kind: "meeting"; meetingId: MeetingId }
  | { kind: "group"; groupId: GroupId }
  | { kind: "all" };

export type ChatRole = "user" | "assistant";

export interface Citation {
  n: number;
  meetingId: MeetingId;
  meetingTitle: string;
  at: MediaMs;
  end: MediaMs;
  snippet: string;
}

export interface ChatMessage {
  id: ChatMessageId;
  threadId: ChatThreadId;
  role: ChatRole;
  content: string;
  citations: Citation[];
  provider: ProviderId | null;
  createdAt: UnixMs;
}

export interface ChatThread {
  id: ChatThreadId;
  scope: ChatScope;
  title: string;
  createdAt: UnixMs;
  updatedAt: UnixMs;
}

export interface ChatDelta {
  threadId: ChatThreadId;
  messageId: ChatMessageId;
  delta: string;
  done: boolean;
  citations: Citation[];
  error: string | null;
}

export interface ChatSendResult {
  userMessage: ChatMessage;
  assistantMessageId: ChatMessageId;
}

// ---------------------------------------------------------------- LLM y ajustes

export interface SecretRef {
  service: string;
  account: string;
}

export type ProviderKind =
  | { type: "claudeCli"; binaryPath: string; model: string | null; effort: string | null; extraEnv: Record<string, string> }
  | { type: "codexCli"; binaryPath: string; model: string | null; extraEnv: Record<string, string> }
  | { type: "opencodeCli"; binaryPath: string; model: string | null; extraEnv: Record<string, string> }
  | { type: "openAiCompatible"; baseUrl: string; model: string; apiKey: SecretRef | null }
  | { type: "anthropic"; model: string; apiKey: SecretRef }
  /** Modelo local integrado: acta lo descarga y lo ejecuta (sin Ollama). */
  | { type: "builtIn"; modelId: string };

export interface ProviderConfig {
  id: ProviderId;
  label: string;
  enabled: boolean;
  kind: ProviderKind;
  timeoutS: number;
}

export interface ProviderTestResult {
  ok: boolean;
  message: string;
  latencyMs: number | null;
}

/** Lo que macOS sabe de quien usa el Mac: para proponer valores en la bienvenida, nunca se aplica solo. */
export interface SystemProfile {
  /** Nombre completo de la cuenta de macOS («Ana García»). `null` si no se pudo leer. */
  fullName: string | null;
  /** Idiomas preferidos del sistema, en orden (`["es-ES", "en-GB"]`). */
  preferredLanguages: string[];
  /** El primero que acta sabe transcribir, listo para `settings.transcription.language`; `"auto"` si ninguno. */
  meetingLanguage: MeetingLanguage;
}

/** Idioma de las reuniones: `"auto"` deja que el transcriptor lo detecte. Dirige la transcripción y el idioma del acta. */
export type MeetingLanguage = "auto" | "es" | "en" | "fr" | "de" | "it" | "pt" | "ca" | (string & {});

/** Una app instalada o en marcha, para elegirla sin conocer su identificador. */
export interface InstalledApp {
  bundleId: string;
  name: string;
  /** Ruta del `.app`. */
  path: string;
  running: boolean;
  /** PNG del icono (ruta absoluta, para `fileUrl`). `null` si no se pudo sacar. */
  iconPath: string | null;
}

export interface CliDetection {
  claude: string | null;
  codex: string | null;
  opencode: string | null;
  claudeEnv: Record<string, string>;
  codexEnv: Record<string, string>;
  opencodeEnv: Record<string, string>;
  opencodeModel?: string | null;
}

/** Un servidor local de modelos (Ollama, LM Studio). */
export interface LocalRuntime {
  running: boolean;
  /** URL base compatible con OpenAI (`…/v1`), lista para un proveedor. */
  baseUrl: string;
  models: string[];
}

/** Espejo de `acta_llm::providers::models::LocalDetection`. */
export interface LocalDetection {
  ollama: LocalRuntime;
  lmStudio: LocalRuntime;
}

// ------------------------------------------------- modelo local integrado
// Espejo de `acta_core::llm::local`.

export type ModelTier = "tiny" | "medium" | "large";

export interface LocalModel {
  /** Es el `modelId` de un proveedor `builtIn`. */
  id: string;
  tier: ModelTier;
  name: string;
  params: string;
  quant: string;
  repo: string;
  file: string;
  revision: string;
  url: string;
  sizeBytes: number;
  sha256: string;
  license: string;
  licenseUrl: string;
  minRamGb: number;
  comfortableRamGb: number;
  ramNeededBytes: number;
  contextTokens: number;
  baseTokensPerS: number;
  description: string;
}

export interface HardwareInfo {
  ramBytes: number;
  /** "Apple M2 Pro". */
  chip: string;
  appleSilicon: boolean;
  freeDiskBytes: number | null;
}

/** Si un modelo cabe en este Mac. `tooBig` y `noDisk` no se pueden descargar. */
export type ModelFit = "comfortable" | "tight" | "tooBig" | "noDisk";

export type RuntimeSource = "env" | "bundled" | "downloaded" | "homebrew";

export interface LocalRuntimeStatus {
  available: boolean;
  source: RuntimeSource | null;
  path: string | null;
  pinnedVersion: string;
  downloadBytes: number;
}

export interface LocalModelStatus {
  model: LocalModel;
  fit: ModelFit;
  recommended: boolean;
  downloaded: boolean;
  /** Bytes de una descarga a medias (se reanuda). */
  partialBytes: number;
  downloading: boolean;
  expectedTokensPerS: number;
}

export interface LocalLlmStatus {
  hardware: HardwareInfo;
  runtime: LocalRuntimeStatus;
  models: LocalModelStatus[];
  recommended: string | null;
  /** Modelo cargado ahora en memoria. */
  running: string | null;
}

export type LocalDownloadStage = "runtime" | "model" | "verifying" | "done";

/** Evento `localmodel:download`. */
export interface LocalLlmDownload {
  modelId: string;
  stage: LocalDownloadStage;
  receivedBytes: number;
  totalBytes: number;
  fraction: number;
  done: boolean;
  error: string | null;
}

/** Un micrófono del sistema (`audio_inputs_list`). */
export interface AudioInput {
  uid: string;
  name: string;
  isDefault: boolean;
  /** "builtin" | "bluetooth" | "usb" | "virtual" | "other" */
  transport: string;
}

export interface ModelStatus {
  name: string;
  downloaded: boolean;
  bytes: number | null;
}

export interface UserIdentity {
  displayName: string;
  aliases: string[];
}

export interface Settings {
  user: UserIdentity;
  recording: {
    segmentSeconds: number;
    frameMs: number;
    micDeviceUid: string | null;
    deleteSegmentsAfterConcat: boolean;
  };
  autoStop: {
    silenceThresholdDbfs: number;
    silencePromptAfterMs: number;
    autoStopAfterUnansweredMs: number;
    maxDurationMs: number;
  };
  health: {
    stallMs: number;
    zerosMs: number;
    segmentGraceMs: number;
    minFreeBytes: number;
  };
  detection: {
    enabled: boolean;
    rules: AppRule[];
    denylist: string[];
    meetUrlPatterns: string[];
    startPromptTimeoutS: number;
    pollMs: number;
    debounceMs: number;
    autoRecord: boolean;
    autoRecordDelayS: number;
  };
  screen: {
    enabled: boolean;
    fps: number;
    phashThreshold: number;
    stableFrames: number;
    candidateEveryS: number;
    ringSeconds: number;
    maxWidth: number;
    keywords: string[];
    pruneCandidates: boolean;
  };
  transcription: {
    language: string;
    diarization: boolean;
    livePreview: boolean;
    voiceprintMinSimilarityPct: number;
    glossary: string[];
    /** Pasada del LLM sobre el transcript (jerga, puntuación); solo actúa si hay proveedor. */
    llmCleanup: boolean;
  };
  llm: {
    providers: ProviderConfig[];
    minutesProvider: ProviderId | null;
    chatProvider: ProviderId | null;
    namingProvider: ProviderId | null;
  };
  embeddings: { model: string };
  /** Bienvenida de primer arranque: se marca al terminarla o saltarla. */
  onboarding: { completed: boolean };
  /** Título de una reunión nueva: Teams → Calendario (opt-in, pide permiso) → LLM. Nunca pisa uno editado. */
  titles: { calendar: boolean };
  /** Sugerencias de borrado de reuniones antiguas (nunca borra sola). */
  cleanup: {
    enabled: boolean;
    /** Antigüedad mínima en días. */
    minAgeDays: number;
    /** «Recordármelo más tarde»: el aviso no vuelve hasta esta fecha. */
    snoozedUntil: UnixMs | null;
  };
}

export type TranscriptFormat = "txt" | "md";

/** Fichero que se acaba de guardar en Descargas (ruta absoluta). */
export interface ExportedFile {
  path: string;
}

export interface AppInfo {
  version: string;
  dataDir: string;
  sidecarVersion: string | null;
  macosVersion: string | null;
}

// ---------------------------------------------------------------- eventos

export interface MeetingUpdated {
  meetingId: MeetingId;
  status: MeetingStatus;
  title: string;
}

/** `main:navigate`: la ventana principal abre esa reunión (`main_window_show{meetingId}`). */
export interface MainNavigate {
  meetingId: MeetingId;
}

export interface ModelDownload {
  name: string;
  fraction: number;
  done: boolean;
  error: string | null;
}

export type ApiErrorCode =
  | "permissionDenied"
  | "notFound"
  | "unavailable"
  | "timeout"
  | "invalidInput"
  | "invalidState"
  | "internal";

export interface ApiError {
  code: ApiErrorCode;
  message: string;
}
