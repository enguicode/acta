// Troceo del audio antes de Parakeet: cortar en las pausas del habla, no en
// ventanas fijas. Sin esto, FluidAudio parte la pista en ventanas de 15 s con
// 2 s de solape y las cose por coincidencia de tokens, y una ventana puede
// empezar o acabar a mitad de palabra. Con Silero VAD cada tramo es una frase
// completa (≤ 14 s) y el silencio entre frases ni se decodifica.
//
// Se copia la idea del segmentador de dictado de Orca (stt-vad-pause-segmenter.ts):
// voz a partir de probabilidad 0,5, una pausa corta la frase, margen antes de la
// voz para no comerse la primera sílaba, tope de longitud que corta en el punto
// más callado. Las constantes cambian porque el Silero de FluidAudio puntúa
// bloques de 256 ms (4096 muestras), no de 32 ms, y es mucho menos tajante:
// medido en pausas de 0,8 a 1,5 s con ruido de sala, la probabilidad solo baja a
// 0,4–0,9, nunca a 0. Por eso no vale el umbral de silencio de 0,35 de Orca (no
// cortaría nunca) ni `VadManager.segmentSpeech` (calibrado a 0,85): se puntúa
// con el VAD y el corte se decide aquí, con dos umbrales:
//   - voz: ≥ 0,5 (permisivo: el habla suave no se descarta, solo se pierde
//     silencio de verdad);
//   - pausa donde cortar: ≥ 3 bloques seguidos por debajo de 0,8 (≈ 0,77 s).
// El tope baja de los 30 s de Orca a 13,5 s porque la ventana de Parakeet v3 en
// FluidAudio es de 15 s: con los márgenes cada tramo cabe en una sola pasada.

import ActaMacProtocol
import AVFoundation
@preconcurrency import FluidAudio
import Foundation

/// Un tramo de habla en milisegundos del fichero original.
struct SpeechSpan: Equatable, Sendable {
    var startMs: UInt64
    var endMs: UInt64
}

/// Región de muestras (16 kHz) con habla.
struct SampleRegion: Equatable, Sendable {
    var start: Int
    var end: Int
    var count: Int { end - start }
}

/// Lo más callado de un bloque: ventana de 20 ms con menos energía.
struct QuietSpot: Equatable, Sendable {
    /// Muestra (absoluta) del centro de la ventana.
    var sample: Int
    var rms: Float
}

enum SpeechPlan {
    static let sampleRate = 16_000
    /// Muestras por bloque que puntúa el VAD.
    static let frame = 4096
    /// Un bloque con al menos esto es voz (Silero: 0,5).
    static let speechProbability: Float = 0.5
    /// Por debajo de esto un bloque cuenta como pausa donde se puede cortar.
    static let pauseProbability: Float = 0.8
    /// Bloques seguidos de pausa que cortan la frase (≈ 0,77 s; Orca: 0,6 s).
    static let pauseFrames = 3
    /// Bloques seguidos sin voz (< 0,5) que cierran una región (≈ 0,5 s).
    static let silenceFrames = 2
    /// Silencio que se conserva antes de la voz para no cortar la primera sílaba (Orca: 0,4 s).
    static let prerollSeconds: Double = 0.5
    /// Cola tras la voz: poca, porque Parakeet alucina sobre el silencio final.
    static let tailSeconds: Double = 0.15
    /// Un tramo de hasta estos bloques (≈ 1 s) sin voz clara se considera ruido.
    static let ghostFrames = 3
    static let maxSpanSeconds: Double = 13.5
    /// Menos que esto no se decodifica (mismo suelo que la pista entera).
    static let minDecodeSeconds: Double = 0.35

    /// Regiones de habla a partir de las probabilidades por bloque.
    static func regions(probabilities probs: [Float], quiet: [QuietSpot] = [], totalSamples: Int) -> [SampleRegion] {
        guard !probs.isEmpty, totalSamples > 0 else { return [] }
        func bounds(_ i: Int) -> (Int, Int) { (min(i * frame, totalSamples), min((i + 1) * frame, totalSamples)) }

        // 1. Regiones con voz: se cierran tras `silenceFrames` bloques sin voz.
        var spans: [(first: Int, last: Int)] = []
        var open: Int?
        var lastActive = 0
        var quietRun = 0
        for (i, p) in probs.enumerated() {
            if p >= speechProbability {
                if open == nil { open = i }
                lastActive = i
                quietRun = 0
            } else if open != nil {
                quietRun += 1
                if quietRun >= silenceFrames {
                    spans.append((open!, lastActive))
                    open = nil
                    quietRun = 0
                }
            }
        }
        if let o = open { spans.append((o, lastActive)) }

        // 2. Dentro de cada región, cortar en las pausas.
        var pieces: [(first: Int, last: Int)] = []
        for r in spans {
            var start = r.first
            var i = r.first
            while i <= r.last {
                guard probs[i] < pauseProbability else { i += 1; continue }
                var j = i
                while j + 1 <= r.last, probs[j + 1] < pauseProbability { j += 1 }
                if j - i + 1 >= pauseFrames, i > start {
                    pieces.append((start, i - 1))
                    start = j + 1
                }
                i = j + 1
            }
            if start <= r.last { pieces.append((start, r.last)) }
        }

        // Un tramo corto sin ningún bloque claro de voz es un soplido o un eco del
        // silencio: Parakeet lo "transcribe" con un "Mm-hmm". Se descarta.
        pieces = pieces.filter { piece in
            piece.last - piece.first + 1 > ghostFrames || (piece.first...piece.last).contains { probs[$0] >= pauseProbability }
        }

        // 3. Tope de longitud: partir por el punto más callado de la ventana. Con
        // energía a 20 ms se corta en un hueco entre palabras; los bloques de 256 ms
        // de la probabilidad solo dicen "más o menos aquí" y caen a mitad de palabra.
        let maxFrames = max(2, Int((maxSpanSeconds * Double(sampleRate)) / Double(frame)))
        var out: [SampleRegion] = []
        for piece in pieces {
            var start = piece.first
            var begin = bounds(start).0
            while piece.last - start + 1 > maxFrames {
                let from = start + maxFrames / 3
                let to = start + maxFrames - 1
                var cut = to
                if quiet.count == probs.count {
                    for i in from...to where (quiet[i].rms, probs[i]) <= (quiet[cut].rms, probs[cut]) { cut = i }
                } else {
                    for i in from...to where probs[i] <= probs[cut] { cut = i }
                }
                let (cs, ce) = bounds(cut)
                let mid = quiet.count == probs.count ? min(max(quiet[cut].sample, cs), ce) : cs + (ce - cs) / 2
                out.append(SampleRegion(start: begin, end: mid))
                begin = mid
                start = cut + 1
            }
            out.append(SampleRegion(start: begin, end: bounds(piece.last).1))
        }
        return out.filter { $0.end > $0.start }
    }

    /// Regiones → tramos en ms con margen (más antes que después), sin solapes
    /// entre vecinos y dentro del fichero.
    static func spans(_ regions: [SampleRegion], duration: Double) -> [SpeechSpan] {
        let rate = Double(sampleRate)
        let raw = regions.map { (start: Double($0.start) / rate, end: Double($0.end) / rate) }
        var padded = raw.map { (start: max(0, $0.start - prerollSeconds), end: min(duration, $0.end + tailSeconds)) }
        for i in raw.indices.dropFirst() where padded[i - 1].end > padded[i].start {
            // Márgenes que se pisan: la cola del anterior se queda con lo que no
            // pasa de la mitad del hueco, el resto es margen del siguiente.
            let gap = max(0, raw[i].start - raw[i - 1].end)
            let boundary = raw[i - 1].end + min(tailSeconds, gap / 2)
            padded[i - 1].end = boundary
            padded[i].start = boundary
        }
        return padded.compactMap { s in
            let start = AsrMapping.ms(s.start)
            let end = AsrMapping.ms(s.end)
            guard end > start, Double(end - start) / 1000 >= minDecodeSeconds else { return nil }
            return SpeechSpan(startMs: start, endMs: end)
        }
    }

    /// Salida de Parakeet para un tramo (tiempos relativos al tramo) → segmentos
    /// en la línea de tiempo del fichero. Los tiempos se desplazan con el inicio
    /// exacto del tramo: el merge y el bleed dependen de ellos.
    static func segments(span: SpeechSpan, tokens: [TimedToken], text: String, confidence: Float) -> [AsrSegmentWire] {
        let offset = Double(span.startMs) / 1000
        guard !tokens.isEmpty else {
            let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !t.isEmpty else { return [] }
            return [AsrSegmentWire(startMs: span.startMs, endMs: span.endMs, text: t, confidence: confidence, words: [])]
        }
        let shifted = tokens.map { TimedToken(text: $0.text, start: $0.start + offset, end: $0.end + offset, confidence: $0.confidence) }
        return AsrMapping.segments(AsrMapping.words(shifted))
    }
}

/// Pasa por el VAD un WAV de cualquier duración leyéndolo por bloques (4 h no
/// caben en memoria) y devuelve las probabilidades de voz por bloque de 256 ms.
enum SpeechScan {
    static func scan(_ url: URL, vad: VadManager) async throws -> (probs: [Float], quiet: [QuietSpot], samples: Int) {
        let file = try AVAudioFile(forReading: url)
        let rate = file.processingFormat.sampleRate
        let resampler = MonoResampler(inRate: rate)
        let chans = Int(file.processingFormat.channelCount)
        let block = AVAudioFrameCount(rate * 30)
        guard let buf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: block) else { return ([], [], 0) }
        let chunk = VadManager.chunkSize
        var pending: [Float] = []
        var probs: [Float] = []
        var quiet: [QuietSpot] = []
        var consumed = 0
        var total = 0
        var state = await vad.makeStreamState()
        func drain(final: Bool) async throws {
            var at = 0
            defer { pending.removeFirst(at) }
            while pending.count - at >= chunk || (final && pending.count > at) {
                let n = min(chunk, pending.count - at)
                let part = Array(pending[at..<(at + n)])
                at += n
                quiet.append(quietest(part, firstSample: consumed))
                consumed += n
                let r = try await vad.processStreamingChunk(part, state: state, config: .default)
                state = r.state
                probs.append(r.probability)
            }
        }
        while file.framePosition < file.length {
            try Task.checkCancellation()
            try file.read(into: buf, frameCount: block)
            guard let ch = buf.floatChannelData else { break }
            let n = Int(buf.frameLength)
            if n == 0 { break }
            var mono = [Float](repeating: 0, count: n)
            for c in 0..<chans { for i in 0..<n { mono[i] += ch[c][i] / Float(chans) } }
            let at16k = resampler.process(mono)
            total += at16k.count
            pending += at16k
            try await drain(final: false)
        }
        try await drain(final: true)
        return (probs, quiet, total)
    }

    /// Ventana de 20 ms (paso de 10 ms) con menos energía dentro del bloque.
    static func quietest(_ part: [Float], firstSample: Int, window: Int = 320, hop: Int = 160) -> QuietSpot {
        guard part.count > window else {
            let e = part.reduce(0) { $0 + $1 * $1 } / Float(max(1, part.count))
            return QuietSpot(sample: firstSample + part.count / 2, rms: e.squareRoot())
        }
        var best = QuietSpot(sample: firstSample, rms: .infinity)
        var i = 0
        while i + window <= part.count {
            var e: Float = 0
            for k in i..<(i + window) { e += part[k] * part[k] }
            let rms = (e / Float(window)).squareRoot()
            if rms < best.rms { best = QuietSpot(sample: firstSample + i + window / 2, rms: rms) }
            i += hop
        }
        return best
    }
}
