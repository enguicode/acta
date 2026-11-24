// Implementación real sobre Tauri v2 (`invoke` + `listen`).
// Los argumentos van en camelCase: Tauri los convierte a snake_case en Rust.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { COMMANDS as C, type ActaApi } from "./contract";

export function createTauriApi(): ActaApi {
  return {
    sessionGetState: () => invoke(C.sessionGetState),
    sessionStart: (app) => invoke(C.sessionStart, { app: app ?? null }),
    sessionPause: () => invoke(C.sessionPause),
    sessionResume: () => invoke(C.sessionResume),
    sessionStop: () => invoke(C.sessionStop),
    sessionDismiss: () => invoke(C.sessionDismiss),
    promptAnswer: (promptId, answer) => invoke(C.promptAnswer, { promptId, answer }),
    captureManual: () => invoke(C.captureManual),

    meetingsList: (query) => invoke(C.meetingsList, { query }),
    meetingGet: (id) => invoke(C.meetingGet, { id }),
    meetingUpdate: (id, title) => invoke(C.meetingUpdate, { id, title }),
    meetingSetNote: (id, note) => invoke(C.meetingSetNote, { id, note }),
    meetingDelete: (id) => invoke(C.meetingDelete, { id }),
    meetingSetFavourite: (id, favourite) => invoke(C.meetingSetFavourite, { id, favourite }),
    meetingReprocess: (request) => invoke(C.meetingReprocess, { request }),
    meetingRetry: (r) =>
      invoke(C.meetingRetry, {
        meetingId: r.meetingId,
        steps: r.steps ?? null,
        providerId: r.providerId ?? null,
        model: r.model ?? null,
      }),
    meetingSearchTranscript: (meetingId, query) => invoke(C.meetingSearchTranscript, { meetingId, query }),
    speakerRename: (meetingId, speakerId, displayName, remember) =>
      invoke(C.speakerRename, { meetingId, speakerId, displayName, remember }),
    actionItemSetDone: (id, done) => invoke(C.actionItemSetDone, { id, done }),
    captureDelete: (id) => invoke(C.captureDelete, { id }),

    groupsList: () => invoke(C.groupsList),
    groupCreate: (name, description) => invoke(C.groupCreate, { name, description: description ?? null }),
    groupUpdate: (id, name, description) =>
      invoke(C.groupUpdate, { id, name: name ?? null, description: description ?? null }),
    groupDelete: (id) => invoke(C.groupDelete, { id }),
    groupSetFavourite: (id, favourite) => invoke(C.groupSetFavourite, { id, favourite }),
    groupAddMeeting: (groupId, meetingId) => invoke(C.groupAddMeeting, { groupId, meetingId }),
    groupRemoveMeeting: (groupId, meetingId) => invoke(C.groupRemoveMeeting, { groupId, meetingId }),
    groupSuggestionsList: () => invoke(C.groupSuggestionsList),
    groupSuggestionsRefresh: () => invoke(C.groupSuggestionsRefresh),
    groupSuggestionAccept: (id, name) => invoke(C.groupSuggestionAccept, { id, name: name ?? null }),
    groupSuggestionDismiss: (id) => invoke(C.groupSuggestionDismiss, { id }),
    groupsSuggestTitle: (groupId) => invoke(C.groupsSuggestTitle, { groupId }),

    liveCancel: (requestId) => invoke(C.liveCancel, { requestId }),
    liveTranscriptGet: (sessionId) => invoke(C.liveTranscriptGet, { sessionId }),
    liveAsk: (r) =>
      invoke(C.liveAsk, { sessionId: r.sessionId, question: r.question, providerId: r.providerId ?? null, model: r.model ?? null }),

    cleanupSuggestions: () => invoke(C.cleanupSuggestions),
    cleanupDelete: (ids) => invoke(C.cleanupDelete, { ids }),
    cleanupKeep: (ids) => invoke(C.cleanupKeep, { ids }),

    search: (query) => invoke(C.search, { query }),
    chatThreadsList: (scope) => invoke(C.chatThreadsList, { scope: scope ?? null }),
    chatThreadCreate: (scope) => invoke(C.chatThreadCreate, { scope }),
    chatThreadDelete: (threadId) => invoke(C.chatThreadDelete, { threadId }),
    chatMessagesList: (threadId) => invoke(C.chatMessagesList, { threadId }),
    chatSend: (threadId, text, llm) => invoke(C.chatSend, { threadId, text, providerId: llm?.providerId ?? null, model: llm?.model ?? null }),
    chatCancel: (requestId) => invoke(C.chatCancel, { requestId }),

    voiceprintsList: () => invoke(C.voiceprintsList),
    voiceprintDelete: (id) => invoke(C.voiceprintDelete, { id }),

    settingsGet: () => invoke(C.settingsGet),
    settingsUpdate: (settings) => invoke(C.settingsUpdate, { settings }),
    llmProviderTest: (providerId) => invoke(C.llmProviderTest, { providerId }),
    llmCliDetect: () => invoke(C.llmCliDetect),
    llmListModels: (baseUrl, providerId) => invoke(C.llmListModels, { baseUrl, providerId: providerId ?? null }),
    llmListOpencodeModels: (binaryPath, providerId) => invoke(C.llmListOpencodeModels, { binaryPath, providerId: providerId ?? null }),
    llmDetectLocal: () => invoke(C.llmDetectLocal),
    llmProvidersAvailable: () => invoke(C.llmProvidersAvailable),
    localLlmStatus: () => invoke(C.localLlmStatus),
    localLlmDownload: (modelId) => invoke(C.localLlmDownload, { modelId }),
    localLlmCancel: (modelId) => invoke(C.localLlmCancel, { modelId }),
    localLlmDelete: (modelId) => invoke(C.localLlmDelete, { modelId }),
    localLlmTest: (modelId) => invoke(C.localLlmTest, { modelId }),
    secretSet: (providerId, value) => invoke(C.secretSet, { providerId, value }),
    secretDelete: (providerId) => invoke(C.secretDelete, { providerId }),
    modelsStatus: () => invoke(C.modelsStatus),
    audioInputsList: () => invoke(C.audioInputsList),
    modelsDownload: (name) => invoke(C.modelsDownload, { name }),
    permissionsStatus: () => invoke(C.permissionsStatus),
    permissionsRequest: (kind) => invoke(C.permissionsRequest, { kind }),
    openSystemSettings: (kind) => invoke(C.openSystemSettings, { kind }),
    healthGet: () => invoke(C.healthGet),
    appInfo: () => invoke(C.appInfo),
    systemProfile: () => invoke(C.systemProfile),
    appsList: () => invoke(C.appsList),
    openExternalUrl: (url) => invoke(C.openExternalUrl, { url }),
    meetingExportTranscript: (meetingId, format) => invoke(C.meetingExportTranscript, { id: meetingId, format }),
    meetingExportAudio: (meetingId) => invoke(C.meetingExportAudio, { id: meetingId }),
    openMeetingFolder: (meetingId) => invoke(C.openMeetingFolder, { id: meetingId }),
    openDataFolder: () => invoke(C.openDataFolder),

    overlaySetExpanded: (expanded) => invoke(C.overlaySetExpanded, { expanded }),
    mainWindowShow: (meetingId) => invoke(C.mainWindowShow, { meetingId: meetingId ?? null }),

    on: (event, handler) => listen(event, (e) => handler(e.payload as never)),
    fileUrl: (absolutePath) => convertFileSrc(absolutePath),
  };
}
