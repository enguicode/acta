// Implementación en memoria de `ActaApi` para tests (vitest) y para
// desarrollar la UI en el navegador sin Tauri. Comportamiento mínimo pero
// coherente con el contrato. `controls.emit` simula eventos del backend.

import type { ActaApi, EventName, EventPayloads, Unsubscribe } from "./contract";
import type * as T from "./types";

export interface MockControls {
  emit<E extends EventName>(event: E, payload: EventPayloads[E]): void;
  /** Simula el ASR en vivo: fusiona por `id` y emite `live:transcript`. */
  pushLive(sessionId: T.MeetingId, segments: T.LiveSegment[]): void;
  /** Estado interno, para aserciones. */
  state: MockState;
}

export interface MockState {
  snapshot: T.SessionSnapshot;
  meetings: T.MeetingDetail[];
  groups: T.Group[];
  suggestions: T.GroupSuggestion[];
  threads: T.ChatThread[];
  messages: T.ChatMessage[];
  voiceprints: T.VoiceprintSummary[];
  settings: T.Settings;
  models: T.ModelStatus[];
  permissions: T.PermissionState[];
  /** Servidores locales de modelos que «hay» en este Mac. */
  local: T.LocalDetection;
  /** Modelos por URL base (sin barra final) para `llmListModels`. */
  remoteModels: Record<string, string[]>;
  /** Modelos devueltos por `llmListOpencodeModels`. */
  opencodeModels: string[];
  /** Enlaces que se han pedido abrir en el navegador. */
  opened: string[];
  /** Descargas pedidas (transcripción / audio) y carpetas abiertas (`data`, `meeting:<id>`). */
  exports: ({ kind: "transcript"; meetingId: string; format: string } | { kind: "audio"; meetingId: string })[];
  folders: string[];
  /** Modelo local integrado: lo que «hay» en este Mac. */
  localLlm: T.LocalLlmStatus;
  /** Reuniones que la limpieza sugeriría borrar. */
  cleanup: T.CleanupCandidate[];
  /** Reuniones con «Mantener». */
  kept: T.MeetingId[];
  /** Peticiones de `meetingRetry`, en orden. */
  retries: T.ReprocessRequest[];
  /** Transcript en vivo por sesión. */
  live: Record<T.MeetingId, T.LiveSegment[]>;
  /** `requestId` de `chatCancel` / `liveCancel`, en orden. */
  cancelled: string[];
  /** Proveedor/modelo pedido en cada `chatSend` (`null` = el de por defecto). */
  chatLlm: (T.LlmOverride | null)[];
  /** Proveedor/modelo pedido en cada `liveAsk`. */
  liveLlm: (T.LlmOverride | null)[];
  systemProfile: T.SystemProfile;
  apps: T.InstalledApp[];
}

/** Minúsculas y sin diacríticos, con el índice UTF-16 original de cada unidad. */
function foldWithMap(text: string): { folded: string; map: number[] } {
  let folded = "";
  const map: number[] = [];
  let i = 0;
  for (const ch of text) {
    const f = ch.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
    for (let k = 0; k < f.length; k++) map.push(i);
    folded += f;
    i += ch.length;
  }
  map.push(i);
  return { folded, map };
}

/** Igual que `meeting_search_transcript` en Rust: sin acentos ni mayúsculas. */
export function searchUtterances(utterances: T.Utterance[], query: string): T.TranscriptMatch[] {
  const q = foldWithMap(query.trim()).folded;
  if (!q) return [];
  return utterances.flatMap((u) => {
    const { folded, map } = foldWithMap(u.text);
    const ranges: [number, number][] = [];
    for (let at = folded.indexOf(q); at >= 0; at = folded.indexOf(q, at + q.length)) {
      ranges.push([map[at]!, map[at + q.length]!]);
    }
    return ranges.length ? [{ utteranceId: u.idx, startMs: u.start, snippet: u.text, ranges }] : [];
  });
}

const CLI_MODELS: Partial<Record<T.ProviderKind["type"], string[]>> = {
  claudeCli: ["sonnet", "opus", "haiku"],
  codexCli: [],
  opencodeCli: [],
};

const GIB = 1024 ** 3;

function localModel(
  tier: T.ModelTier,
  params: string,
  sizeBytes: number,
  ram: [number, number, number],
  speed: [number, number],
  description: string,
): T.LocalModel {
  const repo = `unsloth/Qwen3.5-${params}-GGUF`;
  const file = `Qwen3.5-${params}-Q4_K_M.gguf`;
  const revision = "0".repeat(40);
  return {
    id: `qwen3.5-${params.toLowerCase()}-q4_k_m`,
    tier,
    name: `Qwen3.5 ${params}`,
    params,
    quant: "Q4_K_M",
    repo,
    file,
    revision,
    url: `https://huggingface.co/${repo}/resolve/${revision}/${file}`,
    sizeBytes,
    sha256: "0".repeat(64),
    license: "Apache-2.0",
    licenseUrl: `https://huggingface.co/Qwen/Qwen3.5-${params}`,
    minRamGb: ram[0],
    comfortableRamGb: ram[1],
    ramNeededBytes: ram[2] * GIB,
    contextTokens: speed[0],
    baseTokensPerS: speed[1],
    description,
  };
}

/** El catálogo de `acta_core::llm::local` visto desde un M2 Pro de 32 GB, sin nada descargado. */
export function mockLocalLlmStatus(): T.LocalLlmStatus {
  const models: [T.LocalModel, number][] = [
    [localModel("tiny", "2B", 1_280_835_840, [4, 8, 2.5], [16_384, 45], "El más ligero y rápido. Resúmenes correctos de reuniones cortas."), 78],
    [localModel("medium", "4B", 2_740_937_888, [8, 16, 4.5], [16_384, 24], "Equilibrado. Buen castellano y tareas bien atribuidas."), 41],
    [localModel("large", "9B", 5_680_522_464, [16, 24, 8.5], [24_576, 11], "La mejor calidad sin salir de tu Mac."), 19],
  ];
  return {
    hardware: { ramBytes: 32 * GIB, chip: "Apple M2 Pro", appleSilicon: true, freeDiskBytes: 120 * GIB },
    runtime: { available: false, source: null, path: null, pinnedVersion: "b12444", downloadBytes: 17_011_663 },
    models: models.map(([model, expectedTokensPerS]) => ({
      model,
      fit: "comfortable",
      recommended: model.tier === "large",
      downloaded: false,
      partialBytes: 0,
      downloading: false,
      expectedTokensPerS,
    })),
    recommended: "qwen3.5-9b-q4_k_m",
    running: null,
  };
}

export function noLocalRuntimes(): T.LocalDetection {
  return {
    ollama: { running: false, baseUrl: "http://127.0.0.1:11434/v1", models: [] },
    lmStudio: { running: false, baseUrl: "http://127.0.0.1:1234/v1", models: [] },
  };
}

let counter = 0;
const newId = (): string => {
  counter += 1;
  return `00000000-0000-7000-8000-${counter.toString().padStart(12, "0")}`;
};

const now = (): number => Date.now();

/**
 * Ajustes de un Mac ya configurado (bienvenida vista, un proveedor elegido):
 * es de lo que parten casi todos los tests. Para un primer arranque, `freshSettings`.
 */
export function defaultSettings(): T.Settings {
  return {
    user: { displayName: "Álex Romero", aliases: ["Álex"] },
    recording: { segmentSeconds: 30, frameMs: 100, micDeviceUid: null, deleteSegmentsAfterConcat: false },
    autoStop: {
      silenceThresholdDbfs: -50,
      silencePromptAfterMs: 180_000,
      autoStopAfterUnansweredMs: 120_000,
      maxDurationMs: 14_400_000,
    },
    health: { stallMs: 3_000, zerosMs: 10_000, segmentGraceMs: 10_000, minFreeBytes: 1_000_000_000 },
    detection: {
      enabled: true,
      rules: [{ bundleId: "com.microsoft.teams2", platform: "teams", isBrowser: false }],
      denylist: [],
      meetUrlPatterns: ["meet.google.com/"],
      startPromptTimeoutS: 90,
      pollMs: 1_000,
      debounceMs: 3_000,
      autoRecord: false,
      autoRecordDelayS: 10,
    },
    screen: {
      enabled: false,
      fps: 1,
      phashThreshold: 10,
      stableFrames: 2,
      candidateEveryS: 30,
      ringSeconds: 20,
      maxWidth: 1920,
      keywords: ["importante", "apuntad", "esto es clave", "deadline", "fecha"],
      pruneCandidates: true,
    },
    transcription: { language: "auto", diarization: true, livePreview: true, voiceprintMinSimilarityPct: 75 },
    llm: {
      providers: [
        {
          id: "claude-cli",
          label: "Claude Code (CLI)",
          enabled: true,
          kind: { type: "claudeCli", binaryPath: "/opt/homebrew/bin/claude", model: null, effort: null, extraEnv: {} },
          timeoutS: 600,
        },
      ],
      minutesProvider: "claude-cli",
      chatProvider: "claude-cli",
      namingProvider: "claude-cli",
    },
    embeddings: { model: "intfloat/multilingual-e5-small" },
    // En los tests la bienvenida ya está vista; los suyos la reabren.
    onboarding: { completed: true },
    cleanup: { enabled: true, minAgeDays: 90, snoozedUntil: null },
    titles: { calendar: false },
  };
}

/** Lo que trae una instalación nueva, igual que el backend: sin nombre, idioma automático y ningún proveedor de IA. */
export function freshSettings(): T.Settings {
  const s = defaultSettings();
  return {
    ...s,
    user: { displayName: "", aliases: [] },
    transcription: { ...s.transcription, language: "auto" },
    llm: { providers: [], minutesProvider: null, chatProvider: null, namingProvider: null },
    onboarding: { completed: false },
  };
}

export function sampleMeeting(): T.MeetingDetail {
  const id = newId();
  const me = newId();
  const ana = newId();
  const t0 = now() - 3_600_000;
  return {
    meeting: {
      id,
      title: "Daily del equipo",
      platform: "teams",
      appBundleId: "com.microsoft.teams2",
      startedAt: t0,
      endedAt: t0 + 900_000,
      duration: 900_000,
      status: "ready",
      recovered: false,
      error: null,
      language: "es",
      createdAt: t0,
      updatedAt: t0,
      favourite: false,
    },
    transcript: {
      meetingId: id,
      speakers: [
        {
          id: me,
          meetingId: id,
          label: "Yo",
          displayName: "Edu",
          isMe: true,
          track: "mic",
          diarizationLabel: null,
          nameSource: "me",
          confirmed: true,
          voiceprintId: null,
        },
        {
          id: ana,
          meetingId: id,
          label: "Speaker 1",
          displayName: "Ana",
          isMe: false,
          track: "system",
          diarizationLabel: "S0",
          nameSource: "appUi",
          confirmed: false,
          voiceprintId: null,
        },
      ],
      utterances: [
        { idx: 0, speakerId: ana, track: "system", start: 0, end: 4_000, text: "Empezamos con el plan.", confidence: 0.92 },
        { idx: 1, speakerId: me, track: "mic", start: 4_500, end: 9_000, text: "Yo envío el presupuesto el viernes.", confidence: 0.95 },
      ],
    },
    minutes: {
      meetingId: id,
      summaryMd: "- Se revisó el plan del trimestre.",
      decisions: [{ id: newId(), text: "Lanzar el lunes", at: 60_000 }],
      actionItems: [
        {
          id: newId(),
          meetingId: id,
          text: "Enviar presupuesto",
          ownerName: "Edu",
          ownerSpeakerId: me,
          isMine: true,
          due: "viernes",
          done: false,
          at: 4_500,
          captureIds: [],
        },
      ],
      provider: "claude-cli",
      model: null,
      generatedAt: t0 + 960_000,
    },
    escaleta: {
      temas: [{ titulo: "Plan", resumen: "Revisión del plan", inicio_ms: 0, fin_ms: 900_000, subtemas: [], capturas: [] }],
    },
    captures: [],
    timeline: [{ at: 0, end: 900_000, kind: "topic", label: "Plan", refId: null }],
    groups: [],
    audio: { mic: null, system: null, mix: null },
  };
}

function snapshot(state: T.SessionState, extra: Partial<T.SessionSnapshot> = {}): T.SessionSnapshot {
  return { state, elapsedMs: 0, warnings: [], screenSharing: false, captureCount: 0, at: now(), ...extra };
}

function summaryOf(d: T.MeetingDetail): T.MeetingSummary {
  return {
    id: d.meeting.id,
    title: d.meeting.title,
    platform: d.meeting.platform,
    startedAt: d.meeting.startedAt,
    duration: d.meeting.duration,
    status: d.meeting.status,
    speakerNames: d.transcript.speakers.map((s) => s.displayName ?? s.label),
    myOpenTasks: d.minutes?.actionItems.filter((a) => a.isMine && !a.done).length ?? 0,
    groupIds: d.groups.map((g) => g.id),
    captureCount: d.captures.length,
    snippet: null,
    favourite: d.meeting.favourite,
  };
}

const notFound = (what: string): T.ApiError => ({ code: "notFound", message: `No encontrado: ${what}` });
const invalidState = (message: string): T.ApiError => ({ code: "invalidState", message });

export function createMockApi(seed: Partial<MockState> = {}): { api: ActaApi; controls: MockControls } {
  const state: MockState = {
    snapshot: snapshot({ state: "idle" }),
    meetings: [sampleMeeting()],
    groups: [],
    suggestions: [],
    threads: [],
    messages: [],
    voiceprints: [],
    settings: defaultSettings(),
    models: [
      { name: "asr", downloaded: true, bytes: 600_000_000 },
      { name: "diarizer", downloaded: false, bytes: null },
      { name: "vad", downloaded: false, bytes: null },
    ],
    permissions: [
      { kind: "microphone", status: "granted", target: null },
      { kind: "systemAudio", status: "unknown", target: null },
      { kind: "screenRecording", status: "notDetermined", target: null },
      { kind: "accessibility", status: "notDetermined", target: null },
      { kind: "automation", status: "notDetermined", target: "com.google.Chrome" },
    ],
    local: noLocalRuntimes(),
    remoteModels: {},
    opencodeModels: [],
    opened: [],
    exports: [],
    folders: [],
    localLlm: mockLocalLlmStatus(),
    cleanup: [],
    kept: [],
    retries: [],
    live: {},
    cancelled: [],
    chatLlm: [],
    liveLlm: [],
    systemProfile: { fullName: "Álex Romero", preferredLanguages: ["es-ES", "en-GB"], meetingLanguage: "es" },
    apps: [
      { bundleId: "com.microsoft.teams2", name: "Microsoft Teams", path: "/Applications/Microsoft Teams.app", running: true, iconPath: null },
      { bundleId: "com.apple.Safari", name: "Safari", path: "/Applications/Safari.app", running: false, iconPath: null },
      { bundleId: "com.tinyspeck.slackmacgap", name: "Slack", path: "/Applications/Slack.app", running: false, iconPath: null },
      { bundleId: "com.superduper.superwhisper", name: "superwhisper", path: "/Applications/superwhisper.app", running: true, iconPath: null },
      { bundleId: "com.typeless.app", name: "Typeless", path: "/Applications/Typeless.app", running: true, iconPath: null },
      { bundleId: "us.zoom.xos", name: "zoom.us", path: "/Applications/zoom.us.app", running: false, iconPath: null },
    ],
    ...seed,
  };

  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = <E extends EventName>(event: E, payload: EventPayloads[E]) => {
    listeners.get(event)?.forEach((h) => h(payload));
  };
  const setState = (s: T.SessionState): T.SessionSnapshot => {
    state.snapshot = snapshot(s);
    emit("session:state", state.snapshot);
    return state.snapshot;
  };
  const findMeeting = (id: string): T.MeetingDetail => {
    const m = state.meetings.find((x) => x.meeting.id === id);
    if (!m) throw notFound(`reunión ${id}`);
    return m;
  };
  const findGroup = (id: string): T.Group => {
    const g = state.groups.find((x) => x.id === id);
    if (!g) throw notFound(`grupo ${id}`);
    return g;
  };

  /** Con estrella, o en un grupo con estrella (igual que `favourite_sql` en acta-store). */
  const isFavourite = (m: T.MeetingDetail): boolean =>
    m.meeting.favourite || m.groups.some((g) => state.groups.find((x) => x.id === g.id)?.favourite);
  const localModelStatus = (id: string): T.LocalModelStatus => {
    const m = state.localLlm.models.find((x) => x.model.id === id);
    if (!m) throw notFound(`modelo local ${id}`);
    return m;
  };

  const api: ActaApi = {
    async sessionGetState() {
      return state.snapshot;
    },
    async sessionStart(app) {
      const s = state.snapshot.state.state;
      if (s === "recording" || s === "paused" || s === "stopping") throw invalidState("Ya hay una grabación en curso");
      return setState({ state: "recording", meetingId: newId(), app: app ?? null, startedAt: now(), stopPrompt: null });
    },
    async sessionPause() {
      const s = state.snapshot.state;
      if (s.state !== "recording") throw invalidState("No se está grabando");
      const { state: _ignored, ...rec } = s;
      return setState({ ...rec, state: "paused", pausedAt: now() });
    },
    async sessionResume() {
      const s = state.snapshot.state;
      if (s.state !== "paused") throw invalidState("No está en pausa");
      const { state: _ignored, pausedAt: _p, ...rec } = s;
      return setState({ ...rec, state: "recording" });
    },
    async sessionStop() {
      const s = state.snapshot.state;
      if (s.state !== "recording" && s.state !== "paused") throw invalidState("No se está grabando");
      return setState({ state: "stopping", meetingId: s.meetingId, reason: "user" });
    },
    async sessionDismiss() {
      return setState({ state: "idle" });
    },
    async promptAnswer(promptId, answer) {
      const s = state.snapshot.state;
      if (s.state === "detected" && s.prompt.id === promptId) {
        return answer === "accept"
          ? setState({ state: "recording", meetingId: newId(), app: s.app, startedAt: now(), stopPrompt: null })
          : setState({ state: "idle" });
      }
      if ((s.state === "recording" || s.state === "paused") && s.stopPrompt?.id === promptId) {
        return answer === "accept"
          ? setState({ state: "stopping", meetingId: s.meetingId, reason: "meetingEndedConfirmed" })
          : setState({ ...s, stopPrompt: null });
      }
      throw invalidState("Pregunta caducada");
    },
    async captureManual() {
      return null;
    },

    async meetingsList(query) {
      const text = query.text?.toLowerCase();
      const items = state.meetings
        .filter((m) => !text || m.meeting.title.toLowerCase().includes(text))
        .filter((m) => !query.groupId || m.groups.some((g) => g.id === query.groupId))
        .filter((m) => !query.favouritesOnly || isFavourite(m))
        .map(summaryOf);
      return { items: items.slice(0, query.limit ?? 50), nextCursor: null };
    },
    async meetingGet(id) {
      return findMeeting(id);
    },
    async meetingUpdate(id, title) {
      const m = findMeeting(id);
      m.meeting = { ...m.meeting, title, updatedAt: now() };
      return m.meeting;
    },
    async meetingDelete(id) {
      state.meetings = state.meetings.filter((m) => m.meeting.id !== id);
    },
    async meetingSetFavourite(id, favourite) {
      const m = findMeeting(id);
      m.meeting = { ...m.meeting, favourite };
      emit("meeting:updated", { meetingId: id, status: m.meeting.status, title: m.meeting.title });
      return m.meeting;
    },

    async cleanupSuggestions() {
      const { enabled, minAgeDays } = state.settings.cleanup;
      const candidates = enabled
        ? state.cleanup.filter((c) => {
            const m = state.meetings.find((x) => x.meeting.id === c.meetingId);
            return m && !isFavourite(m) && !state.kept.includes(c.meetingId);
          })
        : [];
      return { candidates, totalBytes: candidates.reduce((n, c) => n + c.bytes, 0), minAgeDays };
    },
    async cleanupDelete(ids) {
      const result: T.CleanupResult = { deleted: [], skipped: [], freedBytes: 0 };
      for (const id of ids) {
        const m = state.meetings.find((x) => x.meeting.id === id);
        if (!m || isFavourite(m)) {
          result.skipped.push(id);
          continue;
        }
        result.freedBytes += state.cleanup.find((c) => c.meetingId === id)?.bytes ?? 0;
        state.meetings = state.meetings.filter((x) => x.meeting.id !== id);
        result.deleted.push(id);
      }
      return result;
    },
    async cleanupKeep(ids) {
      state.kept.push(...ids);
    },
    async meetingReprocess(request) {
      findMeeting(request.meetingId);
    },
    async meetingRetry({ meetingId, steps, providerId, model }) {
      findMeeting(meetingId);
      const llm = providerId ? { providerId, model: model ?? null } : null;
      state.retries.push({ meetingId, steps: steps ?? [], llm });
    },
    async meetingSearchTranscript(meetingId, query) {
      return searchUtterances(findMeeting(meetingId).transcript.utterances, query);
    },
    async speakerRename(meetingId, speakerId, displayName) {
      const m = findMeeting(meetingId);
      const sp = m.transcript.speakers.find((s) => s.id === speakerId);
      if (!sp) throw notFound(`hablante ${speakerId}`);
      sp.displayName = displayName;
      sp.nameSource = "user";
      sp.confirmed = true;
    },
    async actionItemSetDone(id, done) {
      for (const m of state.meetings) {
        const item = m.minutes?.actionItems.find((a) => a.id === id);
        if (item) {
          item.done = done;
          return;
        }
      }
      throw notFound(`tarea ${id}`);
    },
    async captureDelete(id) {
      for (const m of state.meetings) m.captures = m.captures.filter((c) => c.id !== id);
    },

    async groupsList() {
      return state.groups;
    },
    async groupCreate(name, description) {
      const g: T.Group = {
        id: newId(),
        name,
        description: description ?? null,
        kind: "manual",
        meetingCount: 0,
        createdAt: now(),
        updatedAt: now(),
        favourite: false,
      };
      state.groups.push(g);
      return g;
    },
    async groupSetFavourite(id, favourite) {
      const g = findGroup(id);
      g.favourite = favourite;
      return g;
    },
    async groupUpdate(id, name, description) {
      const g = findGroup(id);
      if (name != null) g.name = name;
      if (description != null) g.description = description;
      g.updatedAt = now();
      return g;
    },
    async groupDelete(id) {
      state.groups = state.groups.filter((g) => g.id !== id);
      for (const m of state.meetings) m.groups = m.groups.filter((g) => g.id !== id);
    },
    async groupAddMeeting(groupId, meetingId) {
      const g = findGroup(groupId);
      const m = findMeeting(meetingId);
      if (!m.groups.some((x) => x.id === groupId)) {
        m.groups.push(g);
        g.meetingCount += 1;
      }
    },
    async groupRemoveMeeting(groupId, meetingId) {
      const g = findGroup(groupId);
      const m = findMeeting(meetingId);
      const before = m.groups.length;
      m.groups = m.groups.filter((x) => x.id !== groupId);
      if (m.groups.length < before) g.meetingCount -= 1;
    },
    async groupSuggestionsList() {
      return state.suggestions;
    },
    async groupSuggestionsRefresh() {
      return state.suggestions;
    },
    async groupSuggestionAccept(id, name) {
      const s = state.suggestions.find((x) => x.id === id);
      if (!s) throw notFound(`sugerencia ${id}`);
      state.suggestions = state.suggestions.filter((x) => x.id !== id);
      const g: T.Group = {
        id: s.id,
        name: name ?? s.name ?? "Grupo",
        description: s.description,
        kind: "suggested",
        meetingCount: s.meetingIds.length,
        createdAt: now(),
        updatedAt: now(),
        favourite: false,
      };
      state.groups.push(g);
      return g;
    },
    async groupSuggestionDismiss(id) {
      state.suggestions = state.suggestions.filter((x) => x.id !== id);
    },
    async groupsSuggestTitle(groupId) {
      findGroup(groupId);
      const titles = state.meetings.filter((m) => m.groups.some((g) => g.id === groupId)).map((m) => m.meeting.title);
      return { name: titles[0] ? `Seguimiento: ${titles[0]}` : "Reuniones del equipo", description: null };
    },

    async liveCancel(requestId) {
      state.cancelled.push(requestId);
      emit("live:answer", { requestId, done: true });
    },
    async liveTranscriptGet(sessionId) {
      return { sessionId, segments: structuredClone(state.live[sessionId] ?? []) };
    },
    async liveAsk({ sessionId, question, providerId, model }) {
      state.liveLlm.push(providerId ? { providerId, model: model ?? null } : null);
      const requestId = newId();
      const words = foldWithMap(question).folded.split(/\W+/u).filter((w) => w.length > 4);
      const hits = (state.live[sessionId] ?? []).filter((s) => words.some((w) => foldWithMap(s.text).folded.includes(w)));
      setTimeout(() => {
        const answer = hits.length ? "Sí, se habló de ello " : "No, todavía no se ha hablado de eso.";
        emit("live:answer", { requestId, delta: answer, done: false });
        if (hits.length) emit("live:answer", { requestId, delta: "[01:05].", done: false });
        emit("live:answer", { requestId, done: true, citations: hits.map((s) => ({ startMs: s.startMs })) });
      }, 0);
      return requestId;
    },

    async search(query) {
      const q = query.text.toLowerCase();
      const scope = query.scope;
      const inScope = (m: T.MeetingDetail): boolean =>
        scope.kind === "all" ||
        (scope.kind === "favourites" && isFavourite(m)) ||
        (scope.kind === "meeting" && m.meeting.id === scope.meetingId) ||
        (scope.kind === "group" && m.groups.some((g) => g.id === scope.groupId));
      return state.meetings.filter(inScope).flatMap((m) =>
        m.transcript.utterances
          .filter((u) => u.text.toLowerCase().includes(q))
          .map((u) => ({
            meetingId: m.meeting.id,
            meetingTitle: m.meeting.title,
            startedAt: m.meeting.startedAt,
            at: u.start,
            end: u.end,
            speaker: m.transcript.speakers.find((s) => s.id === u.speakerId)?.displayName ?? null,
            snippet: u.text,
            score: 1,
          })),
      );
    },
    async chatThreadsList(scope) {
      return scope ? state.threads.filter((t) => JSON.stringify(t.scope) === JSON.stringify(scope)) : state.threads;
    },
    async chatThreadCreate(scope) {
      const t: T.ChatThread = { id: newId(), scope, title: "Nueva conversación", createdAt: now(), updatedAt: now() };
      state.threads.push(t);
      return t;
    },
    async chatThreadDelete(threadId) {
      state.threads = state.threads.filter((t) => t.id !== threadId);
      state.messages = state.messages.filter((m) => m.threadId !== threadId);
    },
    async chatMessagesList(threadId) {
      return state.messages.filter((m) => m.threadId === threadId);
    },
    async chatSend(threadId, text, llm) {
      state.chatLlm.push(llm ?? null);
      const userMessage: T.ChatMessage = {
        id: newId(),
        threadId,
        role: "user",
        content: text,
        citations: [],
        provider: null,
        createdAt: now(),
      };
      state.messages.push(userMessage);
      const assistantMessageId = newId();
      const reply = "Respuesta simulada.";
      state.messages.push({
        id: assistantMessageId,
        threadId,
        role: "assistant",
        content: reply,
        citations: [],
        provider: llm?.providerId ?? "mock",
        createdAt: now(),
      });
      queueMicrotask(() =>
        emit("chat:delta", { threadId, messageId: assistantMessageId, delta: reply, done: true, citations: [], error: null }),
      );
      return { userMessage, assistantMessageId };
    },
    async chatCancel(requestId) {
      state.cancelled.push(requestId);
    },

    async voiceprintsList() {
      return state.voiceprints;
    },
    async voiceprintDelete(id) {
      state.voiceprints = state.voiceprints.filter((v) => v.id !== id);
    },

    async settingsGet() {
      return state.settings;
    },
    async settingsUpdate(settings) {
      state.settings = settings;
      return settings;
    },
    async llmProviderTest() {
      return { ok: true, message: "OK (mock)", latencyMs: 1 };
    },
    async systemProfile() {
      return structuredClone(state.systemProfile);
    },
    async appsList() {
      return structuredClone(state.apps);
    },
    async llmCliDetect() {
      return { claude: "/opt/homebrew/bin/claude", codex: null, opencode: null, claudeEnv: {}, codexEnv: {}, opencodeEnv: {}, opencodeModel: null };
    },
    async llmListModels(baseUrl) {
      const key = baseUrl.trim().replace(/\/+$/, "");
      const local = [state.local.ollama, state.local.lmStudio].find((r) => r.running && r.baseUrl === key);
      const models = state.remoteModels[key] ?? local?.models;
      if (!models) throw { code: "unavailable", message: `no se pudo conectar con ${key}/models: ¿está el servidor en marcha?` } satisfies T.ApiError;
      return [...models];
    },
    async llmListOpencodeModels(binaryPath) {
      if (!binaryPath.trim()) {
        throw { code: "invalidInput", message: "Indica primero la ruta de OpenCode en «Avanzado» o pulsa «Detectar CLI instalados»." } satisfies T.ApiError;
      }
      return [...state.opencodeModels];
    },
    async llmDetectLocal() {
      return structuredClone(state.local);
    },
    async llmProvidersAvailable() {
      const { llm } = state.settings;
      return llm.providers
        .filter((p) => p.enabled)
        .map((p): T.AvailableProvider => {
          const configured = "model" in p.kind ? (p.kind.model ?? null) : p.kind.type === "builtIn" ? p.kind.modelId : null;
          const remote = p.kind.type === "openAiCompatible" ? state.remoteModels[p.kind.baseUrl.replace(/\/+$/, "")] : undefined;
          const models = remote ?? CLI_MODELS[p.kind.type] ?? (configured ? [configured] : []);
          const defaultFor = (["minutes", "chat", "naming"] as const).filter((r) => llm[`${r}Provider`] === p.id);
          return { providerId: p.id, label: p.label, kind: p.kind.type, models: [...models], defaultModel: configured, isDefault: llm.minutesProvider === p.id, defaultFor };
        });
    },
    async localLlmStatus() {
      return structuredClone(state.localLlm);
    },
    async localLlmDownload(modelId) {
      const m = localModelStatus(modelId);
      if (m.fit === "tooBig" || m.fit === "noDisk") {
        throw { code: "invalidInput", message: `${m.model.name} no cabe en este Mac.` } satisfies T.ApiError;
      }
      const total = m.model.sizeBytes;
      const event = (stage: T.LocalDownloadStage, fraction: number, done = false): T.LocalLlmDownload => ({
        modelId,
        stage,
        receivedBytes: Math.round(total * fraction),
        totalBytes: total,
        fraction,
        done,
        error: null,
      });
      if (!state.localLlm.runtime.available) emit("localmodel:download", event("runtime", 0.5));
      for (const fraction of [0.3, 0.7]) emit("localmodel:download", event("model", fraction));
      emit("localmodel:download", event("verifying", 1));
      m.downloaded = true;
      m.partialBytes = 0;
      state.localLlm.runtime = { ...state.localLlm.runtime, available: true, source: "downloaded" };
      emit("localmodel:download", event("done", 1, true));
    },
    async localLlmCancel() {},
    async localLlmDelete(modelId) {
      const m = localModelStatus(modelId);
      m.downloaded = false;
      m.partialBytes = 0;
      if (state.localLlm.running === modelId) state.localLlm.running = null;
    },
    async localLlmTest(modelId) {
      const m = localModelStatus(modelId);
      if (!m.downloaded) return { ok: false, message: `El modelo local ${m.model.name} no está descargado.`, latencyMs: null };
      state.localLlm.running = modelId;
      return { ok: true, message: `Conexión correcta (${modelId})`, latencyMs: 1_200 };
    },
    async secretSet(providerId) {
      return { service: "acta", account: `provider:${providerId}` };
    },
    async secretDelete() {},
    async modelsStatus() {
      return state.models.map((m) => ({ ...m }));
    },
    async audioInputsList() {
      return [
        { uid: "BuiltInMicrophoneDevice", name: "Micrófono del MacBook Pro", isDefault: true, transport: "builtin" },
        { uid: "AirPods-input", name: "AirPods", isDefault: false, transport: "bluetooth" },
      ];
    },
    async modelsDownload(name) {
      const model = state.models.find((m) => m.name === name);
      if (!model) throw notFound(`modelo ${name}`);
      for (const fraction of [0.4, 0.8]) emit("model:download", { name, fraction, done: false, error: null });
      model.downloaded = true;
      model.bytes = model.bytes ?? 100_000_000;
      emit("model:download", { name, fraction: 1, done: true, error: null });
    },
    async permissionsStatus() {
      return state.permissions;
    },
    async permissionsRequest(kind) {
      return state.permissions.find((p) => p.kind === kind)?.status ?? "unknown";
    },
    async openSystemSettings() {},
    async healthGet() {
      return state.snapshot.warnings;
    },
    async appInfo() {
      return { version: "0.1.0-mock", dataDir: "/tmp/acta", sidecarVersion: null, macosVersion: null };
    },

    async openExternalUrl(url) {
      state.opened.push(url);
    },
    async meetingExportTranscript(meetingId, format) {
      state.exports.push({ kind: "transcript", meetingId, format });
      return { path: `/tmp/Descargas/transcripcion.${format}` };
    },
    async meetingExportAudio(meetingId) {
      state.exports.push({ kind: "audio", meetingId });
      return { path: "/tmp/Descargas/audio.wav" };
    },
    async openMeetingFolder(meetingId) {
      state.folders.push(`meeting:${meetingId}`);
    },
    async openDataFolder() {
      state.folders.push("data");
    },

    async overlaySetExpanded() {},
    async mainWindowShow(meetingId) {
      if (meetingId) emit("main:navigate", { meetingId });
    },

    async on(event, handler): Promise<Unsubscribe> {
      const set = listeners.get(event) ?? new Set();
      const h = handler as (p: unknown) => void;
      set.add(h);
      listeners.set(event, set);
      return () => {
        set.delete(h);
      };
    },
    fileUrl: (absolutePath) => `file://${absolutePath}`,
  };

  const pushLive = (sessionId: T.MeetingId, segments: T.LiveSegment[]) => {
    const all = (state.live[sessionId] ??= []);
    for (const seg of segments) {
      const i = all.findIndex((x) => x.id === seg.id);
      if (i >= 0) all[i] = seg;
      else all.push(seg);
    }
    emit("live:transcript", { sessionId, segments: structuredClone(segments) });
  };

  return { api, controls: { emit, state, pushLive } };
}
