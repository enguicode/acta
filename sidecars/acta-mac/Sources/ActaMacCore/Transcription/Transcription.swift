// ASR (Parakeet TDT v3) y diarización (pyannote community-1 + WeSpeaker) con
// FluidAudio 0.18.3, offline tras la reunión. Escribe AsrFile /
// DiarizationFile en `out_path` (vía .tmp + rename) y emite `job.progress`.
// Modelos en ~/Library/Application Support/acta/models/fluidaudio.
//
// Notas de FluidAudio 0.18.3 (verificadas en el código, no en la doc):
// - `AsrManager.transcribe` necesita `decoderState: inout TdtDecoderState`.
// - Con URL y > 30 s usa lectura desde disco (vale para 4 h).
// - `Language` solo filtra por alfabeto; es/en decodifican igual.
// - `OfflineDiarizerManager` no es Sendable: se crea uno por trabajo.

import ActaMacProtocol
import AVFoundation
@preconcurrency import FluidAudio
import Foundation

// MARK: - Lógica pura

struct TimedToken: Equatable, Sendable {
    var text: String
    var start: Double
    var end: Double
    var confidence: Float
}

struct TimedWord: Equatable, Sendable {
    var text: String
    var start: Double
    var end: Double
    var confidence: Float
}

enum AsrMapping {
    static let sentenceEnd: Set<Character> = [".", "?", "!", "…", "。"]

    static func ms(_ s: Double) -> UInt64 { UInt64(max(0, (s * 1000).rounded())) }

    /// Tokens SentencePiece → palabras. Un token que empieza por espacio o
    /// "▁" abre palabra; la puntuación suelta se pega a la anterior.
    static func words(_ tokens: [TimedToken]) -> [TimedWord] {
        var out: [TimedWord] = []
        var confs: [Float] = []
        func close() {
            guard !out.isEmpty, !confs.isEmpty else { return }
            out[out.count - 1].confidence = confs.reduce(0, +) / Float(confs.count)
            confs = []
        }
        for t in tokens {
            let startsWord = t.text.hasPrefix(" ") || t.text.hasPrefix("▁")
            let clean = t.text.replacingOccurrences(of: "▁", with: " ").trimmingCharacters(in: .whitespaces)
            if clean.isEmpty { continue }
            let isPunct = clean.unicodeScalars.allSatisfy { CharacterSet.punctuationCharacters.contains($0) }
            if out.isEmpty || (startsWord && !isPunct) {
                close()
                out.append(TimedWord(text: clean, start: t.start, end: t.end, confidence: t.confidence))
            } else {
                out[out.count - 1].text += clean
                out[out.count - 1].end = max(out[out.count - 1].end, t.end)
            }
            confs.append(t.confidence)
        }
        close()
        return out
    }

    /// Palabras → segmentos: corta en fin de frase, en pausas > `maxGap` o
    /// si el segmento supera `maxDuration`.
    static func segments(_ words: [TimedWord], maxGap: Double = 0.8, maxDuration: Double = 15) -> [AsrSegmentWire] {
        var out: [AsrSegmentWire] = []
        var cur: [TimedWord] = []
        func flush() {
            guard let first = cur.first, let last = cur.last else { return }
            let conf = cur.map(\.confidence).reduce(0, +) / Float(cur.count)
            out.append(AsrSegmentWire(
                startMs: ms(first.start), endMs: ms(last.end),
                text: cur.map(\.text).joined(separator: " "),
                confidence: conf,
                words: cur.map { AsrWordWire(startMs: ms($0.start), endMs: ms($0.end), text: $0.text) }
            ))
            cur = []
        }
        for w in words {
            if let last = cur.last, let first = cur.first, w.start - last.end > maxGap || w.end - first.start > maxDuration {
                flush()
            }
            cur.append(w)
            if let c = w.text.last, sentenceEnd.contains(c) { flush() }
        }
        flush()
        return out
    }

    /// Sin tiempos por token: un solo segmento con todo el texto.
    static func fallbackSegment(text: String, duration: Double, confidence: Float) -> [AsrSegmentWire] {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return [] }
        return [AsrSegmentWire(startMs: 0, endMs: ms(duration), text: t, confidence: confidence, words: [])]
    }
}

enum Embeddings {
    static func normalized(_ v: [Float]) -> [Float] {
        let n = sqrt(v.reduce(0) { $0 + $1 * $1 })
        return n > 0 ? v.map { $0 / n } : v
    }

    /// Media de vectores normalizados, renormalizada (para coseno).
    static func meanNormalized(_ vs: [[Float]]) -> [Float] {
        let valid = vs.filter { !$0.isEmpty && $0.allSatisfy(\.isFinite) }
        guard let dim = valid.first?.count else { return [] }
        var acc = [Float](repeating: 0, count: dim)
        for v in valid where v.count == dim {
            for (i, x) in normalized(v).enumerated() { acc[i] += x }
        }
        return normalized(acc)
    }
}

struct DiarSegment: Equatable, Sendable {
    var label: String
    var start: Double
    var end: Double
    var embedding: [Float]
}

enum DiarMapping {
    /// Sin voz en la pista (solo silencio, pitidos o música) no es un fallo:
    /// la reunión se queda sin hablantes diarizados.
    static func isNoSpeech(_ error: Error) -> Bool {
        if case OfflineDiarizationError.noSpeechDetected = error { return true }
        return false
    }

    static func file(_ segs: [DiarSegment], database: [String: [Float]]?, model: String) -> DiarizationFile {
        let sorted = segs.filter { $0.end > $0.start }.sorted { $0.start < $1.start }
        let turns = sorted.map {
            DiarizationTurnWire(label: $0.label, startMs: AsrMapping.ms($0.start), endMs: AsrMapping.ms($0.end))
        }
        var order: [String] = []
        for s in sorted where !order.contains(s.label) { order.append(s.label) }
        let speakers = order.map { label -> DiarizedSpeakerWire in
            let mine = sorted.filter { $0.label == label }
            let total = mine.reduce(0.0) { $0 + ($1.end - $1.start) }
            let emb = database?[label].map(Embeddings.normalized) ?? Embeddings.meanNormalized(mine.map(\.embedding))
            return DiarizedSpeakerWire(label: label, embedding: emb, totalMs: AsrMapping.ms(total))
        }
        return DiarizationFile(embeddingModel: model, turns: turns, speakers: speakers)
    }
}

/// Rutas de modelos bajo `models/fluidaudio`.
struct ModelPaths: Sendable {
    let base: URL
    var asr: URL { base.appendingPathComponent("parakeet-tdt-0.6b-v3", isDirectory: true) }
    var diarizer: URL { base.appendingPathComponent("speaker-diarization", isDirectory: true) }
    /// VadManager añade "Models" por su cuenta.
    var vad: URL { base.appendingPathComponent("Models/silero-vad", isDirectory: true) }

    static var `default`: ModelPaths {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return ModelPaths(base: support.appendingPathComponent("acta/models/fluidaudio", isDirectory: true))
    }

    static func containsFile(named names: [String], under dir: URL) -> Bool {
        guard let e = FileManager.default.enumerator(at: dir, includingPropertiesForKeys: nil) else { return false }
        var found = Set<String>()
        for case let u as URL in e where names.contains(u.lastPathComponent) {
            found.insert(u.lastPathComponent)
            if found.count == names.count { return true }
        }
        return false
    }

    static func size(_ dir: URL) -> UInt64? {
        guard let e = FileManager.default.enumerator(at: dir, includingPropertiesForKeys: [.fileSizeKey]) else { return nil }
        var total: UInt64 = 0
        for case let u as URL in e {
            total += UInt64((try? u.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
        return total > 0 ? total : nil
    }
}

/// Lee solo los tramos pedidos de un WAV (no carga 4 h en memoria).
enum WavSpans {
    static func read(_ url: URL, spans: [SpanMs]) throws -> (samples: [Float], ranges: [Range<TimeInterval>]) {
        let file = try AVAudioFile(forReading: url)
        let rate = file.processingFormat.sampleRate
        let resampler = MonoResampler(inRate: rate)
        var out: [Float] = []
        var ranges: [Range<TimeInterval>] = []
        for s in spans where s.endMs > s.startMs {
            let start = AVAudioFramePosition(Double(s.startMs) / 1000 * rate)
            guard start < file.length else { continue }
            let count = AVAudioFrameCount(min(Double(s.endMs - s.startMs) / 1000 * rate, Double(file.length - start)))
            guard count > 0, let buf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: count) else { continue }
            file.framePosition = start
            try file.read(into: buf, frameCount: count)
            guard let ch = buf.floatChannelData else { continue }
            let n = Int(buf.frameLength)
            let chans = Int(buf.format.channelCount)
            var mono = [Float](repeating: 0, count: n)
            for c in 0..<chans { for i in 0..<n { mono[i] += ch[c][i] / Float(chans) } }
            let at16k = resampler.process(mono)
            let from = Double(out.count) / 16_000
            out += at16k
            ranges.append(from ..< Double(out.count) / 16_000)
        }
        return (out, ranges)
    }

    static func duration(_ url: URL) -> Double {
        guard let f = try? AVAudioFile(forReading: url) else { return 0 }
        return Double(f.length) / f.processingFormat.sampleRate
    }
}

// MARK: - Servicio

/// Id estable del modelo de embeddings (las huellas solo se comparan con el mismo).
let speakerEmbeddingModel = "fluidaudio-wespeaker-community1-256"

public actor TranscriptionService {
    private let output: Output
    private let jobs = JobRegistry()
    private let paths: ModelPaths
    private var asrLoad: Task<(AsrModels, AsrManager), Error>?
    private var diarLoad: Task<OfflineDiarizerModels, Error>?
    private var vadLoad: Task<VadManager, Error>?
    private var live: [TrackName: LiveAsr] = [:]

    public init(output: Output, modelsBase: URL? = nil) {
        self.output = output
        paths = modelsBase.map { ModelPaths(base: $0) } ?? .default
    }

    // MARK: Modelos

    private func progressHandler(_ name: String) -> ProgressHandler {
        let output = output
        let throttle = Throttle()
        return { p in
            guard throttle.shouldEmit(p.fractionCompleted) else { return }
            Task { await output.emit(.modelsProgress(ModelsProgressEvent(name: name, fraction: Float(p.fractionCompleted), done: false, error: nil))) }
        }
    }

    private func asr() async throws -> (AsrModels, AsrManager) {
        if let t = asrLoad { return try await t.value }
        let paths = paths
        let handler = progressHandler("asr")
        let t = Task { () throws -> (AsrModels, AsrManager) in
            let models = try await AsrModels.downloadAndLoad(to: paths.asr, version: .v3, progressHandler: handler)
            let mgr = AsrManager(config: .default)
            try await mgr.loadModels(models)
            return (models, mgr)
        }
        asrLoad = t
        do {
            return try await t.value
        } catch {
            asrLoad = nil
            throw error
        }
    }

    private func vad() async throws -> VadManager {
        if let t = vadLoad { return try await t.value }
        let base = paths.base
        let t = Task { try await VadManager(config: .default, modelDirectory: base) }
        vadLoad = t
        do {
            return try await t.value
        } catch {
            vadLoad = nil
            throw error
        }
    }

    /// Tramos de habla de la pista (Silero VAD, cortando en pausas). `nil` si el
    /// VAD no está disponible: se transcribe la pista entera como antes.
    private func speechSpans(_ wav: URL, duration: Double) async throws -> [SpeechSpan]? {
        // Interruptor para comparar con el troceo de ventanas fijas (A/B en pruebas).
        if ProcessInfo.processInfo.environment["ACTA_ASR_VAD"] == "0" { return nil }
        do {
            let vad = try await self.vad()
            let scan = try await SpeechScan.scan(wav, vad: vad)
            return SpeechPlan.spans(SpeechPlan.regions(probabilities: scan.probs, quiet: scan.quiet, totalSamples: scan.samples), duration: duration)
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            Log.warn("asr: VAD unavailable (\(error)); transcribing the whole track")
            return nil
        }
    }

    private func diarModels() async throws -> OfflineDiarizerModels {
        if let t = diarLoad { return try await t.value }
        let base = paths.base
        let handler = progressHandler("diarizer")
        let t = Task { try await OfflineDiarizerModels.load(from: base, progressHandler: handler) }
        diarLoad = t
        do {
            return try await t.value
        } catch {
            diarLoad = nil
            throw error
        }
    }

    public func modelsStatus() -> ModelsStatusResult {
        let asrOK = AsrModels.modelsExist(at: paths.asr, version: .v3)
        let diarOK = ModelPaths.containsFile(named: ["Segmentation.mlmodelc", "Embedding.mlmodelc"], under: paths.diarizer)
        let vadOK = (try? FileManager.default.contentsOfDirectory(atPath: paths.vad.path).isEmpty == false) ?? false
        return ModelsStatusResult(models: [
            ModelStatusWire(name: "asr", downloaded: asrOK, bytes: ModelPaths.size(paths.asr)),
            ModelStatusWire(name: "diarizer", downloaded: diarOK, bytes: ModelPaths.size(paths.diarizer)),
            ModelStatusWire(name: "vad", downloaded: vadOK, bytes: ModelPaths.size(paths.vad)),
        ])
    }

    /// Responde ya; el progreso va por `models.progress`.
    public func download(_ name: String) throws {
        guard ["asr", "diarizer", "vad"].contains(name) else {
            throw ProtocolError(code: .invalidArgs, message: "unknown model \(name)")
        }
        Task {
            do {
                switch name {
                case "asr": _ = try await self.asr()
                case "diarizer": _ = try await self.diarModels()
                default:
                    _ = try await VadManager(config: .default, modelDirectory: self.paths.base, progressHandler: self.progressHandler("vad"))
                }
                await self.output.emit(.modelsProgress(ModelsProgressEvent(name: name, fraction: 1, done: true, error: nil)))
            } catch {
                await self.output.emit(.modelsProgress(ModelsProgressEvent(name: name, fraction: 0, done: false, error: "\(error)")))
            }
        }
    }

    // MARK: Trabajos

    private func progress(_ job: String, _ f: Double, _ stage: String) async {
        await output.emit(.jobProgress(JobProgressEvent(jobId: job, fraction: Float(min(1, max(0, f))), stage: stage)))
    }

    public func cancel(_ jobId: String) async throws {
        guard await jobs.cancel(jobId) else { throw ProtocolError(code: .notFound, message: "no job \(jobId)") }
    }

    public func cancelAll() async { await jobs.cancelAll() }

    public func transcribe(_ a: AsrTranscribeArgs) async throws -> AsrTranscribeResult {
        guard a.model == "parakeet-tdt-0.6b-v3" else {
            throw ProtocolError(code: .invalidArgs, message: "unsupported model \(a.model)")
        }
        let wav = URL(fileURLWithPath: a.wavPath)
        guard FileManager.default.fileExists(atPath: wav.path) else {
            throw ProtocolError(code: .notFound, message: "wav not found: \(a.wavPath)")
        }
        return try await jobs.run(a.jobId) {
            await self.progress(a.jobId, 0, "loading")
            let duration = WavSpans.duration(wav)
            var segments: [AsrSegmentWire] = []
            if duration >= 0.35 {
                let (_, mgr) = try await self.asr()
                try Task.checkCancellation()
                let lang = a.language.flatMap { Language(rawValue: String($0.prefix(2)).lowercased()) }
                await self.progress(a.jobId, 0.02, "segmenting")
                if let spans = try await self.speechSpans(wav, duration: duration) {
                    let spoken = spans.reduce(0.0) { $0 + Double($1.endMs - $1.startMs) / 1000 }
                    Log.info("asr: \(spans.count) speech spans (\(Int(spoken)) s of \(Int(duration)) s) in \(wav.lastPathComponent)")
                    await self.progress(a.jobId, 0.1, "transcribing")
                    for (i, span) in spans.enumerated() {
                        try Task.checkCancellation()
                        let (samples, _) = try WavSpans.read(wav, spans: [SpanMs(startMs: span.startMs, endMs: span.endMs)])
                        guard !samples.isEmpty else { continue }
                        var state = TdtDecoderState.make(decoderLayers: await mgr.decoderLayerCount)
                        let r = try await mgr.transcribe(samples, decoderState: &state, language: lang)
                        let tokens = (r.tokenTimings ?? []).map {
                            TimedToken(text: $0.token, start: $0.startTime, end: $0.endTime, confidence: $0.confidence)
                        }
                        segments += SpeechPlan.segments(span: span, tokens: tokens, text: r.text, confidence: r.confidence)
                        await self.progress(a.jobId, 0.1 + 0.85 * Double(i + 1) / Double(spans.count), "transcribing")
                    }
                } else {
                    await self.progress(a.jobId, 0.05, "transcribing")
                    let stream = await mgr.transcriptionProgressStream
                    let reporter = Task {
                        for try await p in stream { await self.progress(a.jobId, 0.05 + 0.9 * p, "transcribing") }
                    }
                    defer { reporter.cancel() }
                    var state = TdtDecoderState.make(decoderLayers: await mgr.decoderLayerCount)
                    let r = try await mgr.transcribe(wav, decoderState: &state, language: lang)
                    let tokens = (r.tokenTimings ?? []).map {
                        TimedToken(text: $0.token, start: $0.startTime, end: $0.endTime, confidence: $0.confidence)
                    }
                    segments = tokens.isEmpty
                        ? AsrMapping.fallbackSegment(text: r.text, duration: duration, confidence: r.confidence)
                        : AsrMapping.segments(AsrMapping.words(tokens))
                }
            }
            try Task.checkCancellation()
            let file = AsrFile(model: a.model, language: a.language, durationMs: AsrMapping.ms(duration), segments: segments)
            try AtomicFile.writeJSON(file, to: URL(fileURLWithPath: a.outPath))
            await self.progress(a.jobId, 1, "done")
            return AsrTranscribeResult(outPath: a.outPath, segments: UInt32(segments.count), durationMs: AsrMapping.ms(duration))
        }
    }

    public func diarize(_ a: DiarizeRunArgs) async throws -> DiarizeRunResult {
        let wav = URL(fileURLWithPath: a.wavPath)
        guard FileManager.default.fileExists(atPath: wav.path) else {
            throw ProtocolError(code: .notFound, message: "wav not found: \(a.wavPath)")
        }
        return try await jobs.run(a.jobId) {
            await self.progress(a.jobId, 0, "loading")
            let duration = WavSpans.duration(wav)
            var segs: [DiarSegment] = []
            var database: [String: [Float]]?
            if duration >= 1 {
                let models = try await self.diarModels()
                try Task.checkCancellation()
                var cfg = OfflineDiarizerConfig.default
                cfg.clustering.numSpeakers = a.numSpeakers.map(Int.init)
                cfg.clustering.minSpeakers = a.minSpeakers.map(Int.init)
                cfg.clustering.maxSpeakers = a.maxSpeakers.map(Int.init)
                cfg.postProcessing.exclusiveSegments = true
                let manager = OfflineDiarizerManager(config: cfg)
                manager.initialize(models: models)
                let jobId = a.jobId
                let output = self.output
                do {
                    let r = try await manager.process(wav) { done, total in
                        guard total > 0 else { return }
                        let f = 0.05 + 0.9 * Double(done) / Double(total)
                        Task { await output.emit(.jobProgress(JobProgressEvent(jobId: jobId, fraction: Float(f), stage: "diarizing"))) }
                    }
                    segs = r.segments.map {
                        DiarSegment(label: $0.speakerId, start: Double($0.startTimeSeconds), end: Double($0.endTimeSeconds), embedding: $0.embedding)
                    }
                    database = r.speakerDatabase
                } catch where DiarMapping.isNoSpeech(error) {
                    Log.info("diarize: no speech in \(wav.lastPathComponent), 0 speakers")
                }
            }
            try Task.checkCancellation()
            let file = DiarMapping.file(segs, database: database, model: speakerEmbeddingModel)
            try AtomicFile.writeJSON(file, to: URL(fileURLWithPath: a.outPath))
            await self.progress(a.jobId, 1, "done")
            return DiarizeRunResult(outPath: a.outPath, speakers: UInt32(file.speakers.count))
        }
    }

    public func embed(_ a: VoiceprintEmbedArgs) async throws -> VoiceprintEmbedResult {
        let (samples, ranges) = try WavSpans.read(URL(fileURLWithPath: a.wavPath), spans: a.spans)
        guard !ranges.isEmpty else { throw ProtocolError(code: .invalidArgs, message: "no valid spans") }
        let models = try await diarModels()
        let manager = OfflineDiarizerManager(config: .default)
        manager.initialize(models: models)
        let embs = try await manager.embedSpans(audioSource: ArrayAudioSampleSource(samples: samples), spans: ranges)
        let mean = Embeddings.meanNormalized(embs)
        guard !mean.isEmpty else { throw ProtocolError(code: .internalError, message: "empty embedding") }
        return VoiceprintEmbedResult(embedding: mean, model: speakerEmbeddingModel)
    }

    // MARK: Vivo

    /// Devuelve el sumidero para CaptureService.
    public func liveStart(_ a: AsrLiveStartArgs) async throws -> @Sendable (TrackName, [Float]) -> Void {
        await liveStop()
        let (models, _) = try await asr()
        let lang = a.language.flatMap { Language(rawValue: String($0.prefix(2)).lowercased()) }
        for track in Set(a.tracks) {
            let session = try await LiveAsr.start(track: track, models: models, language: lang, output: output)
            live[track] = session
        }
        let sessions = live
        return { track, samples in sessions[track]?.feed(samples) }
    }

    public func liveStop() async {
        for s in live.values { await s.stop() }
        live = [:]
    }
}

/// Una pista en vivo: FIFO de audio → SlidingWindowAsrManager → asr.partial.
final class LiveAsr: Sendable {
    private let manager: SlidingWindowAsrManager
    private let continuation: AsyncStream<[Float]>.Continuation
    private let feeder: Task<Void, Never>
    private let updates: Task<Void, Never>

    private init(manager: SlidingWindowAsrManager, continuation: AsyncStream<[Float]>.Continuation, feeder: Task<Void, Never>, updates: Task<Void, Never>) {
        self.manager = manager
        self.continuation = continuation
        self.feeder = feeder
        self.updates = updates
    }

    static func start(track: TrackName, models: AsrModels, language: Language?, output: Output) async throws -> LiveAsr {
        let mgr = SlidingWindowAsrManager(config: SlidingWindowAsrConfig.streaming.applying(language: language))
        try await mgr.loadModels(models)
        try await mgr.startStreaming(source: track == .mic ? .microphone : .system)
        let (stream, cont) = AsyncStream<[Float]>.makeStream(bufferingPolicy: .bufferingNewest(200))
        let feeder = Task {
            for await samples in stream {
                // Formato y buffer nacen en cada vuelta: así forman una región aislada que se
                // puede enviar al actor sin riesgo de carrera (Swift 6.1 lo exige).
                guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
                      let buf = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)) else { continue }
                buf.frameLength = AVAudioFrameCount(samples.count)
                samples.withUnsafeBufferPointer { buf.floatChannelData![0].update(from: $0.baseAddress!, count: samples.count) }
                await mgr.streamAudio(buf)
            }
        }
        let updatesStream = await mgr.transcriptionUpdates
        let updates = Task {
            for await u in updatesStream {
                let text = u.text.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !text.isEmpty else { continue }
                await output.emit(.asrPartial(AsrPartialEvent(track: track, text: text, atUnixMs: Clock.unixMs(), isFinal: u.isConfirmed)))
            }
        }
        return LiveAsr(manager: mgr, continuation: cont, feeder: feeder, updates: updates)
    }

    func feed(_ samples: [Float]) { continuation.yield(samples) }

    func stop() async {
        continuation.finish()
        feeder.cancel()
        _ = try? await manager.finish()
        updates.cancel()
    }
}

/// Limita eventos de progreso a cambios ≥ 1 %.
final class Throttle: @unchecked Sendable {
    private let lock = NSLock()
    private var last: Double = -1
    func shouldEmit(_ f: Double) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if f - last >= 0.01 || f >= 1 { last = f; return true }
        return false
    }
}
