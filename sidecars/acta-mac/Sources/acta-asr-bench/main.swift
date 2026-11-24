// acta-asr-bench: compara motores de ASR sobre un WAV de 16 kHz mono.
// Uso: acta-asr-bench <parakeet|parakeet-es|nemotron-es|nemotron-auto> <wav> [chunkMs]
// Imprime una línea JSON: {engine, seconds, audioSeconds, text}.
// Los modelos van a ~/Library/Application Support/acta/models/fluidaudio.
import AVFoundation
import FluidAudio
import Foundation

func samples(_ url: URL) throws -> [Float] {
    let file = try AVAudioFile(forReading: url)
    let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!
    guard file.processingFormat.sampleRate == 16_000, file.processingFormat.channelCount == 1 else {
        fatalError("se espera 16 kHz mono")
    }
    let buf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length))!
    try file.read(into: buf)
    _ = fmt
    return Array(UnsafeBufferPointer(start: buf.floatChannelData![0], count: Int(buf.frameLength)))
}

let args = CommandLine.arguments
guard args.count >= 3 else {
    print("uso: acta-asr-bench <parakeet|parakeet-es|nemotron-es|nemotron-auto> <wav> [chunkMs]")
    exit(2)
}
let engine = args[1]
let wav = URL(fileURLWithPath: args[2])
let chunkMs = args.count > 3 ? Int(args[3]) ?? 2240 : 2240
let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    .appendingPathComponent("acta/models/fluidaudio", isDirectory: true)
let audio = try samples(wav)
let audioSeconds = Double(audio.count) / 16_000

var text = ""
let loadStart = Date()
var t0 = Date()
switch engine {
case "parakeet", "parakeet-es":
    let models = try await AsrModels.downloadAndLoad(to: base.appendingPathComponent("parakeet-tdt-0.6b-v3"), version: .v3)
    let mgr = AsrManager(config: .default)
    try await mgr.loadModels(models)
    t0 = Date()
    var state = TdtDecoderState.make(decoderLayers: await mgr.decoderLayerCount)
    let r = try await mgr.transcribe(audio, decoderState: &state, language: engine == "parakeet-es" ? .spanish : nil)
    text = r.text
case "vad":
    // Diagnóstico: probabilidades de Silero por bloque de 256 ms y tramos (config de Orca).
    let vad = try await VadManager(config: .default, modelDirectory: base)
    let results = try await vad.process(audio)
    let probs = results.map { String(format: "%.2f", $0.probability) }.joined(separator: " ")
    let env = ProcessInfo.processInfo.environment
    let neg = Float(env["VAD_NEG"] ?? "") ?? 0.7
    let sil = Double(env["VAD_SIL"] ?? "") ?? 0.5
    let segs = await vad.segmentSpeech(from: results, totalSamples: audio.count, config: VadSegmentationConfig(
        minSpeechDuration: 0.25, minSilenceDuration: sil, maxSpeechDuration: 13.5, speechPadding: 0,
        silenceThresholdForSplit: neg, negativeThreshold: neg, negativeThresholdOffset: 0.15))
    print(probs)
    print(segs.map { String(format: "%.2f-%.2f", $0.startTime, $0.endTime) }.joined(separator: " "))
    exit(0)
case "nemotron-es", "nemotron-auto":
    let lang = engine == "nemotron-es" ? "es-ES" : "auto"
    let shared = try await StreamingNemotronMultilingualAsrManager.downloadAndPreloadShared(
        languageCode: lang, chunkMs: chunkMs, to: base.appendingPathComponent("nemotron-multilingual"))
    let mgr = StreamingNemotronMultilingualAsrManager()
    try await mgr.loadFromShared(shared)
    if engine == "nemotron-es" {
        await mgr.setLanguage("es-ES")
        await mgr.setForcedPrefix(true)
    }
    t0 = Date()
    _ = try await mgr.process(samples: audio)
    let r = try await mgr.finishWithTokenTimings()
    text = r.text
default:
    print("motor desconocido: \(engine)")
    exit(2)
}
let secs = Date().timeIntervalSince(t0)
let load = t0.timeIntervalSince(loadStart)
let out: [String: Any] = ["engine": engine, "seconds": secs, "loadSeconds": load, "audioSeconds": audioSeconds, "text": text]
let data = try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
