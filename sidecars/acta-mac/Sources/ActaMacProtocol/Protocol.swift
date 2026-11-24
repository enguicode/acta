// Protocolo sidecar ⇄ Rust, versión 1.
// Espejo EXACTO de crates/acta-mac-bridge/src/protocol.rs.
// Especificación: docs/01-arquitectura.md §4.
//
// - stdin  (Rust → sidecar): {"id":7,"cmd":"capture.start","args":{...}}  (sin "args" si no hay)
// - stdout (sidecar → Rust): {"type":"response",...} | {"type":"event","ts":...,"event":"...","data":{...}}
// - stderr: logs en texto libre. NUNCA protocolo por stderr ni logs por stdout.
//
// Claves snake_case vía JSONEncoder/Decoder (.convertToSnakeCase / .convertFromSnakeCase).
// OJO con los acrónimos: la propiedad Swift debe ser `bundleIds`, no `bundleIDs`.

import Foundation

public let protocolVersion: UInt32 = 1
/// Identificador del modelo de embeddings de voz (huella de voz y diarización).
/// Espejo de `SPEAKER_EMBEDDING_MODEL` en protocol.rs.
public let speakerEmbeddingModel = "fluidaudio-wespeaker-community1-256"
public let maxLineBytes = 1024 * 1024

public enum ProtocolCoding {
    public static func decoder() -> JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    public static func encoder() -> JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        e.outputFormatting = [.withoutEscapingSlashes]
        return e
    }
}

// MARK: - Enums compartidos

public enum TrackName: String, Codable, Sendable { case mic, system }

public enum PermissionName: String, Codable, Sendable {
    case microphone, systemAudio = "system_audio", screenRecording = "screen_recording", accessibility, automation
}

public enum PermissionStatusName: String, Codable, Sendable {
    case granted, denied, notDetermined = "not_determined", unknown
}

public enum CaptureReasonName: String, Codable, Sendable {
    case slideChange = "slide_change", keyword, llmPick = "llm_pick", manual, candidate
}

public enum TrackStateName: String, Codable, Sendable { case running, stalled, restarting, stopped, failed }

public enum SignalName: String, Codable, Sendable {
    case micInUse = "mic_in_use", micReleased = "mic_released"
    case meetingWindowOpen = "meeting_window_open", meetingWindowClosed = "meeting_window_closed"
    case meetTabOpen = "meet_tab_open", meetTabClosed = "meet_tab_closed"
    case audioPidsChanged = "audio_pids_changed"
}

public enum ErrorCode: String, Codable, Sendable {
    case permissionDenied = "permission_denied", notFound = "not_found", unsupported
    case invalidArgs = "invalid_args", busy, cancelled, internalError = "internal"
}

// MARK: - Argumentos de comandos

public struct HelloArgs: Codable, Sendable, Equatable {
    public var protocolVersion: UInt32
    public init(protocolVersion: UInt32) {
        self.protocolVersion = protocolVersion
    }
}

public struct PermissionsRequestArgs: Codable, Sendable, Equatable {
    public var kind: PermissionName
    public var target: String?

    public init(kind: PermissionName, target: String? = nil) {
        self.kind = kind
        self.target = target
    }
}

public struct AppRuleWire: Codable, Sendable, Equatable {
    public var bundleId: String
    public var platform: String
    public var isBrowser: Bool

    public init(bundleId: String, platform: String, isBrowser: Bool) {
        self.bundleId = bundleId
        self.platform = platform
        self.isBrowser = isBrowser
    }
}

public struct DetectStartArgs: Codable, Sendable, Equatable {
    public var rules: [AppRuleWire]
    public var meetUrlPatterns: [String]
    public var pollMs: UInt32

    public init(rules: [AppRuleWire], meetUrlPatterns: [String], pollMs: UInt32) {
        self.rules = rules
        self.meetUrlPatterns = meetUrlPatterns
        self.pollMs = pollMs
    }
}

public struct MicArgs: Codable, Sendable, Equatable {
    public var enabled: Bool
    public var deviceUid: String?

    public init(enabled: Bool, deviceUid: String? = nil) {
        self.enabled = enabled
        self.deviceUid = deviceUid
    }
}

public struct SystemArgs: Codable, Sendable, Equatable {
    public var enabled: Bool
    public var pids: [Int32]
    public var bundleIds: [String]

    public init(enabled: Bool, pids: [Int32], bundleIds: [String]) {
        self.enabled = enabled
        self.pids = pids
        self.bundleIds = bundleIds
    }
}

public struct CaptureStartArgs: Codable, Sendable, Equatable {
    public var sessionId: String
    public var sampleRate: UInt32
    public var frameMs: UInt32
    public var mic: MicArgs
    public var system: SystemArgs

    public init(sessionId: String, sampleRate: UInt32, frameMs: UInt32, mic: MicArgs, system: SystemArgs) {
        self.sessionId = sessionId
        self.sampleRate = sampleRate
        self.frameMs = frameMs
        self.mic = mic
        self.system = system
    }
}

public struct CaptureRetargetArgs: Codable, Sendable, Equatable {
    public var pids: [Int32]
    public var bundleIds: [String]

    public init(pids: [Int32], bundleIds: [String]) {
        self.pids = pids
        self.bundleIds = bundleIds
    }
}

public struct SpeakersWatchArgs: Codable, Sendable, Equatable {
    public var pid: Int32
    public var bundleId: String

    public init(pid: Int32, bundleId: String) {
        self.pid = pid
        self.bundleId = bundleId
    }
}

public struct ScreenStartArgs: Codable, Sendable, Equatable {
    public var sessionId: String
    public var pid: Int32
    public var bundleId: String
    public var windowTitleHint: String?
    public var outDir: String
    public var candidatesDir: String
    public var fps: Float
    public var phashThreshold: UInt32
    public var stableFrames: UInt32
    public var candidateEveryS: UInt32
    public var ringSeconds: UInt32
    public var maxWidth: UInt32
    public var format: String

    public init(sessionId: String, pid: Int32, bundleId: String, windowTitleHint: String? = nil, outDir: String, candidatesDir: String, fps: Float, phashThreshold: UInt32, stableFrames: UInt32, candidateEveryS: UInt32, ringSeconds: UInt32, maxWidth: UInt32, format: String) {
        self.sessionId = sessionId
        self.pid = pid
        self.bundleId = bundleId
        self.windowTitleHint = windowTitleHint
        self.outDir = outDir
        self.candidatesDir = candidatesDir
        self.fps = fps
        self.phashThreshold = phashThreshold
        self.stableFrames = stableFrames
        self.candidateEveryS = candidateEveryS
        self.ringSeconds = ringSeconds
        self.maxWidth = maxWidth
        self.format = format
    }
}

/// `clipboard.start`: cada imagen nueva del portapapeles → `clipboard.image`.
public struct ClipboardStartArgs: Codable, Sendable, Equatable {
    public var outDir: String
    public var maxWidth: UInt32

    public init(outDir: String, maxWidth: UInt32) {
        self.outDir = outDir
        self.maxWidth = maxWidth
    }
}

public struct ScreenSnapshotArgs: Codable, Sendable, Equatable {
    public var reason: CaptureReasonName
    public var atUnixMs: Int64
    public var keyword: String?

    public init(reason: CaptureReasonName, atUnixMs: Int64, keyword: String? = nil) {
        self.reason = reason
        self.atUnixMs = atUnixMs
        self.keyword = keyword
    }
}

public struct AsrTranscribeArgs: Codable, Sendable, Equatable {
    public var jobId: String
    public var wavPath: String
    public var outPath: String
    public var language: String?
    public var model: String

    public init(jobId: String, wavPath: String, outPath: String, language: String? = nil, model: String) {
        self.jobId = jobId
        self.wavPath = wavPath
        self.outPath = outPath
        self.language = language
        self.model = model
    }
}

public struct AsrLiveStartArgs: Codable, Sendable, Equatable {
    public var tracks: [TrackName]
    public var language: String?

    public init(tracks: [TrackName], language: String? = nil) {
        self.tracks = tracks
        self.language = language
    }
}

public struct DiarizeRunArgs: Codable, Sendable, Equatable {
    public var jobId: String
    public var wavPath: String
    public var outPath: String
    public var numSpeakers: UInt8?
    public var minSpeakers: UInt8?
    public var maxSpeakers: UInt8?

    public init(jobId: String, wavPath: String, outPath: String, numSpeakers: UInt8? = nil, minSpeakers: UInt8? = nil, maxSpeakers: UInt8? = nil) {
        self.jobId = jobId
        self.wavPath = wavPath
        self.outPath = outPath
        self.numSpeakers = numSpeakers
        self.minSpeakers = minSpeakers
        self.maxSpeakers = maxSpeakers
    }
}

public struct SpanMs: Codable, Sendable, Equatable {
    public var startMs: UInt64
    public var endMs: UInt64

    public init(startMs: UInt64, endMs: UInt64) {
        self.startMs = startMs
        self.endMs = endMs
    }
}

public struct VoiceprintEmbedArgs: Codable, Sendable, Equatable {
    public var wavPath: String
    public var spans: [SpanMs]

    public init(wavPath: String, spans: [SpanMs]) {
        self.wavPath = wavPath
        self.spans = spans
    }
}

public struct ModelsDownloadArgs: Codable, Sendable, Equatable {
    public var name: String
    public init(name: String) {
        self.name = name
    }
}

public struct JobCancelArgs: Codable, Sendable, Equatable {
    public var jobId: String
    public init(jobId: String) {
        self.jobId = jobId
    }
}

// MARK: - Comandos

public enum Command: Sendable, Equatable {
    case hello(HelloArgs)
    case shutdown
    case permissionsStatus
    case permissionsRequest(PermissionsRequestArgs)
    case devicesList
    case appsList
    case detectStart(DetectStartArgs)
    case detectStop
    case captureStart(CaptureStartArgs)
    case capturePause
    case captureResume
    case captureStop
    case captureRetarget(CaptureRetargetArgs)
    case speakersWatch(SpeakersWatchArgs)
    case speakersUnwatch
    case screenStart(ScreenStartArgs)
    case screenSnapshot(ScreenSnapshotArgs)
    case screenStop
    case clipboardStart(ClipboardStartArgs)
    case clipboardStop
    case asrTranscribe(AsrTranscribeArgs)
    case asrLiveStart(AsrLiveStartArgs)
    case asrLiveStop
    case diarizeRun(DiarizeRunArgs)
    case voiceprintEmbed(VoiceprintEmbedArgs)
    case modelsStatus
    case modelsDownload(ModelsDownloadArgs)
    case jobCancel(JobCancelArgs)
    case meetingTitles(MeetingTitlesArgs)
    case calendarEvents(CalendarEventsArgs)
    /// Nombre de la cuenta e idiomas preferidos de macOS (para la bienvenida).
    case systemProfile
    /// Apps instaladas y en marcha, con su icono en PNG.
    case appsInstalled(AppsInstalledArgs)
}

public struct SystemProfileResult: Codable, Sendable, Equatable {
    /// `NSFullUserName()`. Falta si está vacío.
    public var fullName: String?
    /// `Locale.preferredLanguages`, en orden ("es-ES").
    public var preferredLanguages: [String]
    public init(fullName: String?, preferredLanguages: [String]) {
        self.fullName = fullName
        self.preferredLanguages = preferredLanguages
    }
}

public struct AppsInstalledArgs: Codable, Sendable, Equatable {
    /// Carpeta donde dejar `<bundle id>.png` (se crea; no se reescribe un
    /// icono que ya está y es más nuevo que la app).
    public var iconsDir: String
    /// Lado del PNG en píxeles.
    public var iconPx: UInt32
    public init(iconsDir: String, iconPx: UInt32) {
        self.iconsDir = iconsDir
        self.iconPx = iconPx
    }
}

public struct InstalledAppWire: Codable, Sendable, Equatable {
    public var bundleId: String
    public var name: String
    /// Ruta del `.app`.
    public var path: String
    public var running: Bool
    /// Ruta absoluta del PNG. Falta si no se pudo sacar el icono.
    public var iconPath: String?
    public init(bundleId: String, name: String, path: String, running: Bool, iconPath: String?) {
        self.bundleId = bundleId
        self.name = name
        self.path = path
        self.running = running
        self.iconPath = iconPath
    }
}

public struct AppsInstalledResult: Codable, Sendable, Equatable {
    public var apps: [InstalledAppWire]
    public init(apps: [InstalledAppWire]) { self.apps = apps }
}

public struct MeetingTitlesArgs: Codable, Sendable, Equatable {
    public var pid: Int32
    public var bundleId: String
    public init(pid: Int32, bundleId: String) {
        self.pid = pid
        self.bundleId = bundleId
    }
}

public struct MeetingTitlesResult: Codable, Sendable, Equatable {
    public var titles: [String]
    public init(titles: [String]) { self.titles = titles }
}

public struct CalendarEventsArgs: Codable, Sendable, Equatable {
    public var fromUnixMs: Int64
    public var toUnixMs: Int64
    public var requestAccess: Bool
    public init(fromUnixMs: Int64, toUnixMs: Int64, requestAccess: Bool) {
        self.fromUnixMs = fromUnixMs
        self.toUnixMs = toUnixMs
        self.requestAccess = requestAccess
    }
}

public struct CalendarEventWire: Codable, Sendable, Equatable {
    public var title: String
    public var startUnixMs: Int64
    public var endUnixMs: Int64
    public var allDay: Bool
    public init(title: String, startUnixMs: Int64, endUnixMs: Int64, allDay: Bool) {
        self.title = title
        self.startUnixMs = startUnixMs
        self.endUnixMs = endUnixMs
        self.allDay = allDay
    }
}

public struct CalendarEventsResult: Codable, Sendable, Equatable {
    /// granted | denied | not_determined
    public var access: String
    public var events: [CalendarEventWire]
    public init(access: String, events: [CalendarEventWire]) {
        self.access = access
        self.events = events
    }
}

public enum RequestDecodingError: Error, Equatable {
    /// El comando no existe; se conoce el id para responder con error.
    case unknownCommand(id: UInt64, cmd: String)
}

/// Una línea de stdin.
public struct Request: Decodable, Sendable, Equatable {
    public let id: UInt64
    public let command: Command

    enum CodingKeys: String, CodingKey { case id, cmd, args }

    public init(id: UInt64, command: Command) {
        self.id = id
        self.command = command
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UInt64.self, forKey: .id)
        let cmd = try c.decode(String.self, forKey: .cmd)
        func args<T: Decodable>(_: T.Type) throws -> T { try c.decode(T.self, forKey: .args) }
        switch cmd {
        case "hello": command = .hello(try args(HelloArgs.self))
        case "shutdown": command = .shutdown
        case "permissions.status": command = .permissionsStatus
        case "permissions.request": command = .permissionsRequest(try args(PermissionsRequestArgs.self))
        case "devices.list": command = .devicesList
        case "apps.list": command = .appsList
        case "detect.start": command = .detectStart(try args(DetectStartArgs.self))
        case "detect.stop": command = .detectStop
        case "capture.start": command = .captureStart(try args(CaptureStartArgs.self))
        case "capture.pause": command = .capturePause
        case "capture.resume": command = .captureResume
        case "capture.stop": command = .captureStop
        case "capture.retarget": command = .captureRetarget(try args(CaptureRetargetArgs.self))
        case "speakers.watch": command = .speakersWatch(try args(SpeakersWatchArgs.self))
        case "speakers.unwatch": command = .speakersUnwatch
        case "screen.start": command = .screenStart(try args(ScreenStartArgs.self))
        case "screen.snapshot": command = .screenSnapshot(try args(ScreenSnapshotArgs.self))
        case "screen.stop": command = .screenStop
        case "clipboard.start": command = .clipboardStart(try args(ClipboardStartArgs.self))
        case "clipboard.stop": command = .clipboardStop
        case "asr.transcribe": command = .asrTranscribe(try args(AsrTranscribeArgs.self))
        case "asr.live.start": command = .asrLiveStart(try args(AsrLiveStartArgs.self))
        case "asr.live.stop": command = .asrLiveStop
        case "diarize.run": command = .diarizeRun(try args(DiarizeRunArgs.self))
        case "voiceprint.embed": command = .voiceprintEmbed(try args(VoiceprintEmbedArgs.self))
        case "models.status": command = .modelsStatus
        case "models.download": command = .modelsDownload(try args(ModelsDownloadArgs.self))
        case "job.cancel": command = .jobCancel(try args(JobCancelArgs.self))
        case "meeting.titles": command = .meetingTitles(try args(MeetingTitlesArgs.self))
        case "calendar.events": command = .calendarEvents(try args(CalendarEventsArgs.self))
        case "system.profile": command = .systemProfile
        case "apps.installed": command = .appsInstalled(try args(AppsInstalledArgs.self))
        default: throw RequestDecodingError.unknownCommand(id: id, cmd: cmd)
        }
    }
}

// MARK: - Respuestas

public struct ProtocolError: Codable, Sendable, Equatable, Error {
    public var code: ErrorCode
    public var message: String
    public init(code: ErrorCode, message: String) {
        self.code = code
        self.message = message
    }
}

/// `{"type":"response","id":..,"ok":..,"result"?:..,"error"?:..}`
public struct ResponseLine<R: Encodable & Sendable>: Encodable, Sendable {
    public let type = "response"
    public var id: UInt64
    public var ok: Bool
    public var result: R?
    public var error: ProtocolError?

    public static func success(_ id: UInt64, _ result: R) -> Self {
        .init(id: id, ok: true, result: result, error: nil)
    }

    enum CodingKeys: String, CodingKey { case type, id, ok, result, error }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(type, forKey: .type)
        try c.encode(id, forKey: .id)
        try c.encode(ok, forKey: .ok)
        try c.encodeIfPresent(result, forKey: .result)
        try c.encodeIfPresent(error, forKey: .error)
    }
}

/// Resultado vacío `{}`.
public struct Empty: Codable, Sendable, Equatable { public init() {} }

public extension ResponseLine where R == Empty {
    static func failure(_ id: UInt64, _ error: ProtocolError) -> Self {
        .init(id: id, ok: false, result: nil, error: error)
    }
}

public struct HelloResult: Codable, Sendable, Equatable {
    public var sidecarVersion: String
    public var protocolVersion: UInt32
    public var macosVersion: String
    public var capabilities: [String]
    public init(sidecarVersion: String, protocolVersion: UInt32, macosVersion: String, capabilities: [String]) {
        self.sidecarVersion = sidecarVersion
        self.protocolVersion = protocolVersion
        self.macosVersion = macosVersion
        self.capabilities = capabilities
    }
}

public struct PermissionEntry: Codable, Sendable, Equatable {
    public var kind: PermissionName
    public var status: PermissionStatusName
    public var target: String?

    public init(kind: PermissionName, status: PermissionStatusName, target: String? = nil) {
        self.kind = kind
        self.status = status
        self.target = target
    }
}

public struct PermissionsStatusResult: Codable, Sendable, Equatable {
    public var permissions: [PermissionEntry]
    public init(permissions: [PermissionEntry]) {
        self.permissions = permissions
    }
}
public struct PermissionsRequestResult: Codable, Sendable, Equatable {
    public var status: PermissionStatusName
    public init(status: PermissionStatusName) {
        self.status = status
    }
}

public struct DeviceWire: Codable, Sendable, Equatable {
    public var uid: String
    public var name: String
    public var isDefault: Bool
    public var transport: String

    public init(uid: String, name: String, isDefault: Bool, transport: String) {
        self.uid = uid
        self.name = name
        self.isDefault = isDefault
        self.transport = transport
    }
}

public struct DevicesListResult: Codable, Sendable, Equatable {
    public var inputs: [DeviceWire]
    public var outputs: [DeviceWire]

    public init(inputs: [DeviceWire], outputs: [DeviceWire]) {
        self.inputs = inputs
        self.outputs = outputs
    }
}

public struct AppInfoWire: Codable, Sendable, Equatable {
    public var pid: Int32
    public var bundleId: String
    public var name: String
    public var audioPids: [Int32]

    public init(pid: Int32, bundleId: String, name: String, audioPids: [Int32]) {
        self.pid = pid
        self.bundleId = bundleId
        self.name = name
        self.audioPids = audioPids
    }
}

public struct AppsListResult: Codable, Sendable, Equatable {
    public var apps: [AppInfoWire]
    public init(apps: [AppInfoWire]) {
        self.apps = apps
    }
}

public struct CaptureStartResult: Codable, Sendable, Equatable {
    public var startedAtUnixMs: Int64
    public var micDevice: String?
    public var systemPids: [Int32]

    public init(startedAtUnixMs: Int64, micDevice: String? = nil, systemPids: [Int32]) {
        self.startedAtUnixMs = startedAtUnixMs
        self.micDevice = micDevice
        self.systemPids = systemPids
    }
}

public struct ScreenFrameEvent: Codable, Sendable, Equatable {
    public var path: String
    public var atUnixMs: Int64
    public var reason: CaptureReasonName
    public var phash: String
    public var width: UInt32
    public var height: UInt32
    public var keyword: String?
    /// Miniatura gris 64×36 en hex (4608 chars): el gate de Rust la
    /// compara con la de la última captura conservada. Opcional (sidecars viejos).
    public var thumb: String?

    public init(path: String, atUnixMs: Int64, reason: CaptureReasonName, phash: String, width: UInt32, height: UInt32, keyword: String? = nil, thumb: String? = nil) {
        self.path = path
        self.atUnixMs = atUnixMs
        self.reason = reason
        self.phash = phash
        self.width = width
        self.height = height
        self.keyword = keyword
        self.thumb = thumb
    }
}

public struct ScreenSnapshotResult: Codable, Sendable, Equatable {
    public var frame: ScreenFrameEvent?
    public init(frame: ScreenFrameEvent? = nil) {
        self.frame = frame
    }
}

public struct AsrTranscribeResult: Codable, Sendable, Equatable {
    public var outPath: String
    public var segments: UInt32
    public var durationMs: UInt64

    public init(outPath: String, segments: UInt32, durationMs: UInt64) {
        self.outPath = outPath
        self.segments = segments
        self.durationMs = durationMs
    }
}

public struct DiarizeRunResult: Codable, Sendable, Equatable {
    public var outPath: String
    public var speakers: UInt32

    public init(outPath: String, speakers: UInt32) {
        self.outPath = outPath
        self.speakers = speakers
    }
}

public struct VoiceprintEmbedResult: Codable, Sendable, Equatable {
    public var embedding: [Float]
    public var model: String

    public init(embedding: [Float], model: String) {
        self.embedding = embedding
        self.model = model
    }
}

public struct ModelStatusWire: Codable, Sendable, Equatable {
    public var name: String
    public var downloaded: Bool
    public var bytes: UInt64?

    public init(name: String, downloaded: Bool, bytes: UInt64? = nil) {
        self.name = name
        self.downloaded = downloaded
        self.bytes = bytes
    }
}

public struct ModelsStatusResult: Codable, Sendable, Equatable {
    public var models: [ModelStatusWire]
    public init(models: [ModelStatusWire]) {
        self.models = models
    }
}

// MARK: - Eventos

public struct AudioFrameEvent: Codable, Sendable, Equatable {
    public var track: TrackName
    public var seq: UInt64
    public var ptsUs: UInt64
    public var sampleRate: UInt32
    public var channels: UInt16
    public var format: String
    /// PCM s16le en base64.
    public var data: String

    public init(track: TrackName, seq: UInt64, ptsUs: UInt64, sampleRate: UInt32, channels: UInt16, format: String, data: String) {
        self.track = track
        self.seq = seq
        self.ptsUs = ptsUs
        self.sampleRate = sampleRate
        self.channels = channels
        self.format = format
        self.data = data
    }
}

public struct AudioDeviceChangedEvent: Codable, Sendable, Equatable {
    public var track: TrackName
    public var deviceUid: String?
    public var deviceName: String?
    public var reason: String

    public init(track: TrackName, deviceUid: String? = nil, deviceName: String? = nil, reason: String) {
        self.track = track
        self.deviceUid = deviceUid
        self.deviceName = deviceName
        self.reason = reason
    }
}

public struct AudioTrackStateEvent: Codable, Sendable, Equatable {
    public var track: TrackName
    public var state: TrackStateName
    public var message: String?

    public init(track: TrackName, state: TrackStateName, message: String? = nil) {
        self.track = track
        self.state = state
        self.message = message
    }
}

public struct AudioOverrunEvent: Codable, Sendable, Equatable {
    public var track: TrackName
    public var droppedFrames: UInt64

    public init(track: TrackName, droppedFrames: UInt64) {
        self.track = track
        self.droppedFrames = droppedFrames
    }
}

public struct DetectSignalEvent: Codable, Sendable, Equatable {
    public var signal: SignalName
    public var app: AppInfoWire
    public var windowTitle: String?
    public var url: String?

    public init(signal: SignalName, app: AppInfoWire, windowTitle: String? = nil, url: String? = nil) {
        self.signal = signal
        self.app = app
        self.windowTitle = windowTitle
        self.url = url
    }
}

public struct ParticipantWire: Codable, Sendable, Equatable {
    public var name: String
    public var isSelf: Bool
    public var muted: Bool?

    public init(name: String, isSelf: Bool, muted: Bool? = nil) {
        self.name = name
        self.isSelf = isSelf
        self.muted = muted
    }
}

public struct SpeakersRosterEvent: Codable, Sendable, Equatable {
    public var pid: Int32
    public var participants: [ParticipantWire]

    public init(pid: Int32, participants: [ParticipantWire]) {
        self.pid = pid
        self.participants = participants
    }
}

public struct SpeakersActiveEvent: Codable, Sendable, Equatable {
    public var pid: Int32
    public var names: [String]

    public init(pid: Int32, names: [String]) {
        self.pid = pid
        self.names = names
    }
}

public struct ScreenShareStateEvent: Codable, Sendable, Equatable {
    public var sharing: Bool
    public var presenter: String?
    public var source: String

    public init(sharing: Bool, presenter: String? = nil, source: String) {
        self.sharing = sharing
        self.presenter = presenter
        self.source = source
    }
}

public struct AsrPartialEvent: Codable, Sendable, Equatable {
    public var track: TrackName
    public var text: String
    public var atUnixMs: Int64
    public var isFinal: Bool

    public init(track: TrackName, text: String, atUnixMs: Int64, isFinal: Bool) {
        self.track = track
        self.text = text
        self.atUnixMs = atUnixMs
        self.isFinal = isFinal
    }
}

public struct JobProgressEvent: Codable, Sendable, Equatable {
    public var jobId: String
    public var fraction: Float
    public var stage: String?

    public init(jobId: String, fraction: Float, stage: String? = nil) {
        self.jobId = jobId
        self.fraction = fraction
        self.stage = stage
    }
}

public struct ModelsProgressEvent: Codable, Sendable, Equatable {
    public var name: String
    public var fraction: Float
    public var done: Bool
    public var error: String?

    public init(name: String, fraction: Float, done: Bool, error: String? = nil) {
        self.name = name
        self.fraction = fraction
        self.done = done
        self.error = error
    }
}

public struct PermissionsChangedEvent: Codable, Sendable, Equatable {
    public var kind: PermissionName
    public var status: PermissionStatusName

    public init(kind: PermissionName, status: PermissionStatusName) {
        self.kind = kind
        self.status = status
    }
}

public struct LogEvent: Codable, Sendable, Equatable {
    public var level: String
    public var message: String

    public init(level: String, message: String) {
        self.level = level
        self.message = message
    }
}

public struct FatalEvent: Codable, Sendable, Equatable {
    public var message: String
    public init(message: String) {
        self.message = message
    }
}

public enum Event: Sendable, Equatable {
    case audioFrame(AudioFrameEvent)
    case audioDeviceChanged(AudioDeviceChangedEvent)
    case audioTrackState(AudioTrackStateEvent)
    case audioOverrun(AudioOverrunEvent)
    case detectSignal(DetectSignalEvent)
    case speakersRoster(SpeakersRosterEvent)
    case speakersActive(SpeakersActiveEvent)
    case screenShareState(ScreenShareStateEvent)
    case screenFrame(ScreenFrameEvent)
    /// Imagen del portapapeles ya guardada como JPEG (`reason: manual`).
    case clipboardImage(ScreenFrameEvent)
    case asrPartial(AsrPartialEvent)
    case jobProgress(JobProgressEvent)
    case modelsProgress(ModelsProgressEvent)
    case permissionsChanged(PermissionsChangedEvent)
    case log(LogEvent)
    case fatal(FatalEvent)

    public var name: String {
        switch self {
        case .audioFrame: "audio.frame"
        case .audioDeviceChanged: "audio.device_changed"
        case .audioTrackState: "audio.track_state"
        case .audioOverrun: "audio.overrun"
        case .detectSignal: "detect.signal"
        case .speakersRoster: "speakers.roster"
        case .speakersActive: "speakers.active"
        case .screenShareState: "screen.share_state"
        case .screenFrame: "screen.frame"
        case .clipboardImage: "clipboard.image"
        case .asrPartial: "asr.partial"
        case .jobProgress: "job.progress"
        case .modelsProgress: "models.progress"
        case .permissionsChanged: "permissions.changed"
        case .log: "log"
        case .fatal: "fatal"
        }
    }
}

/// `{"type":"event","ts":<unix ms>,"event":"<name>","data":{...}}`
public struct EventLine: Encodable, Sendable {
    public var ts: Int64
    public var event: Event

    public init(ts: Int64, event: Event) {
        self.ts = ts
        self.event = event
    }

    enum CodingKeys: String, CodingKey { case type, ts, event, data }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode("event", forKey: .type)
        try c.encode(ts, forKey: .ts)
        try c.encode(event.name, forKey: .event)
        switch event {
        case .audioFrame(let d): try c.encode(d, forKey: .data)
        case .audioDeviceChanged(let d): try c.encode(d, forKey: .data)
        case .audioTrackState(let d): try c.encode(d, forKey: .data)
        case .audioOverrun(let d): try c.encode(d, forKey: .data)
        case .detectSignal(let d): try c.encode(d, forKey: .data)
        case .speakersRoster(let d): try c.encode(d, forKey: .data)
        case .speakersActive(let d): try c.encode(d, forKey: .data)
        case .screenShareState(let d): try c.encode(d, forKey: .data)
        case .screenFrame(let d): try c.encode(d, forKey: .data)
        case .clipboardImage(let d): try c.encode(d, forKey: .data)
        case .asrPartial(let d): try c.encode(d, forKey: .data)
        case .jobProgress(let d): try c.encode(d, forKey: .data)
        case .modelsProgress(let d): try c.encode(d, forKey: .data)
        case .permissionsChanged(let d): try c.encode(d, forKey: .data)
        case .log(let d): try c.encode(d, forKey: .data)
        case .fatal(let d): try c.encode(d, forKey: .data)
        }
    }
}

// MARK: - Ficheros de salida de trabajos largos

public struct AsrWordWire: Codable, Sendable, Equatable {
    public var startMs: UInt64
    public var endMs: UInt64
    public var text: String

    public init(startMs: UInt64, endMs: UInt64, text: String) {
        self.startMs = startMs
        self.endMs = endMs
        self.text = text
    }
}

public struct AsrSegmentWire: Codable, Sendable, Equatable {
    public var startMs: UInt64
    public var endMs: UInt64
    public var text: String
    public var confidence: Float?
    public var words: [AsrWordWire]

    public init(startMs: UInt64, endMs: UInt64, text: String, confidence: Float? = nil, words: [AsrWordWire]) {
        self.startMs = startMs
        self.endMs = endMs
        self.text = text
        self.confidence = confidence
        self.words = words
    }
}

/// `transcript/asr-<track>.json`
public struct AsrFile: Codable, Sendable, Equatable {
    public var model: String
    public var language: String?
    public var durationMs: UInt64
    public var segments: [AsrSegmentWire]

    public init(model: String, language: String? = nil, durationMs: UInt64, segments: [AsrSegmentWire]) {
        self.model = model
        self.language = language
        self.durationMs = durationMs
        self.segments = segments
    }
}

public struct DiarizationTurnWire: Codable, Sendable, Equatable {
    public var label: String
    public var startMs: UInt64
    public var endMs: UInt64

    public init(label: String, startMs: UInt64, endMs: UInt64) {
        self.label = label
        self.startMs = startMs
        self.endMs = endMs
    }
}

public struct DiarizedSpeakerWire: Codable, Sendable, Equatable {
    public var label: String
    public var embedding: [Float]
    public var totalMs: UInt64

    public init(label: String, embedding: [Float], totalMs: UInt64) {
        self.label = label
        self.embedding = embedding
        self.totalMs = totalMs
    }
}

/// `transcript/diarization.json`
public struct DiarizationFile: Codable, Sendable, Equatable {
    public var embeddingModel: String
    public var turns: [DiarizationTurnWire]
    public var speakers: [DiarizedSpeakerWire]

    public init(embeddingModel: String, turns: [DiarizationTurnWire], speakers: [DiarizedSpeakerWire]) {
        self.embeddingModel = embeddingModel
        self.turns = turns
        self.speakers = speakers
    }
}
