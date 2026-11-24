import ActaMacProtocol
import Foundation
import Testing
@testable import ActaMacCore

private let F = SpeechPlan.frame

/// `n` bloques (256 ms) con la misma probabilidad.
private func run(_ p: Float, _ n: Int) -> [Float] { Array(repeating: p, count: n) }

private func regions(_ probs: [Float], extra: Int = 0) -> [SampleRegion] {
    SpeechPlan.regions(probabilities: probs, totalSamples: probs.count * F + extra)
}

@Test func `a long silence between phrases cuts two regions and drops the silence`() {
    let r = regions(run(1, 4) + run(0.05, 6) + run(1, 4))
    #expect(r == [SampleRegion(start: 0, end: 4 * F), SampleRegion(start: 10 * F, end: 14 * F)])
}

@Test func `a pause the silero model only dips to 0,5 still cuts the phrase`() {
    // Medido con ruido de sala: una pausa de 1,2 s deja 0,62 0,50 0,33, nunca 0.
    let r = regions(run(1, 5) + [0.62, 0.55, 0.6] + run(1, 5))
    #expect(r.count == 2)
    #expect(r[0].end == 5 * F && r[1].start == 8 * F)
}

@Test func `a dip shorter than the pause does not cut a phrase`() {
    #expect(regions(run(1, 5) + [0.6, 0.6] + run(1, 5)).count == 1)
}

@Test func `three frames that stay above the pause level do not cut`() {
    #expect(regions(run(1, 5) + run(0.85, 3) + run(1, 5)).count == 1)
}

@Test func `soft speech between 0,5 and 0,8 is kept and never dropped as silence`() {
    let r = regions(run(0.6, 10))
    #expect(r == [SampleRegion(start: 0, end: 10 * F)])
}

@Test func `a single quiet frame inside speech does not close the region`() {
    #expect(regions(run(1, 4) + run(0.1, 1) + run(1, 4)).count == 1)
}

@Test func `silence only gives no regions`() {
    #expect(regions(run(0.05, 20)).isEmpty)
    #expect(SpeechPlan.regions(probabilities: [], totalSamples: 0).isEmpty)
}

@Test func `the last region ends at the real end of the file`() {
    let r = regions(run(1, 3), extra: -1000) // último bloque corto
    #expect(r == [SampleRegion(start: 0, end: 3 * F - 1000)])
}

@Test func `continuous speech longer than the cap splits at its quietest block`() {
    var probs = run(1, 70) // 17,9 s seguidos
    probs[40] = 0.3 // el punto más callado de la ventana
    let r = regions(probs)
    #expect(r.count == 2)
    #expect(r[0].start == 0)
    // corta a mitad del bloque 40 y el siguiente tramo arranca justo ahí
    #expect(r[0].end == 40 * F + F / 2 && r[1].start == r[0].end)
    #expect(r[1].end == 70 * F)
    #expect(r.allSatisfy { Double($0.count) / 16_000 <= SpeechPlan.maxSpanSeconds })
}

@Test func `no region exceeds the cap however long the speech`() {
    let r = regions(run(1, 400)) // 102 s sin una pausa
    #expect(r.count >= 8)
    #expect(r.allSatisfy { Double($0.count) / 16_000 <= SpeechPlan.maxSpanSeconds })
    #expect(r.first?.start == 0 && r.last?.end == 400 * F)
    for (a, b) in zip(r, r.dropFirst()) { #expect(a.end == b.start) } // sin huecos ni solapes
}

@Test func `spans add more preroll than tail and keep exact milliseconds`() {
    let rate = 16_000
    let s = SpeechPlan.spans([SampleRegion(start: 10 * rate, end: 14 * rate)], duration: 60)
    #expect(s == [SpeechSpan(startMs: 9_500, endMs: 14_150)])
}

@Test func `spans clamp to the file`() {
    let s = SpeechPlan.spans([SampleRegion(start: 4_000, end: 59 * 16_000 + 12_000)], duration: 59.5)
    #expect(s == [SpeechSpan(startMs: 0, endMs: 59_500)])
}

@Test func `neighbouring margins never overlap and the tail wins at most half the gap`() {
    let rate = 16_000
    // hueco de 0,6 s entre regiones: el margen (0,5 + 0,15) no cabe
    let s = SpeechPlan.spans([
        SampleRegion(start: 0, end: 2 * rate), SampleRegion(start: 2 * rate + 9_600, end: 5 * rate),
    ], duration: 10)
    #expect(s.count == 2)
    #expect(s[0].endMs == s[1].startMs)
    #expect(s[0].endMs == 2_150) // cola completa (0,15 < 0,3)
    // hueco de 0,2 s: la cola se queda con la mitad
    let t = SpeechPlan.spans([
        SampleRegion(start: 0, end: 2 * rate), SampleRegion(start: 2 * rate + 3_200, end: 5 * rate),
    ], duration: 10)
    #expect(t[0].endMs == 2_100 && t[1].startMs == 2_100)
}

@Test func `spans drop slivers too short to decode`() {
    #expect(SpeechPlan.spans([SampleRegion(start: 16_000, end: 16_000 + 800)], duration: 10).count == 1) // con margen cabe
    #expect(SpeechPlan.spans([SampleRegion(start: 0, end: 800)], duration: 0.2).isEmpty)
}

@Test func `a span's tokens land on the file timeline`() {
    let span = SpeechSpan(startMs: 12_000, endMs: 15_000)
    let tokens = [
        TimedToken(text: " Hola", start: 0.2, end: 0.6, confidence: 0.9),
        TimedToken(text: " todos.", start: 0.6, end: 1.1, confidence: 0.7),
        TimedToken(text: " Sigamos", start: 2.0, end: 2.5, confidence: 0.8),
    ]
    let segs = SpeechPlan.segments(span: span, tokens: tokens, text: "Hola todos. Sigamos", confidence: 0.8)
    #expect(segs.map(\.text) == ["Hola todos.", "Sigamos"])
    #expect(segs[0].startMs == 12_200 && segs[0].endMs == 13_100)
    #expect(segs[0].words.map(\.startMs) == [12_200, 12_600])
    #expect(segs[1].startMs == 14_000 && segs[1].endMs == 14_500)
}

@Test func `a span without token timings becomes one segment covering it`() {
    let span = SpeechSpan(startMs: 4_000, endMs: 6_500)
    let segs = SpeechPlan.segments(span: span, tokens: [], text: " hola ", confidence: 0.6)
    #expect(segs.count == 1)
    #expect(segs[0].startMs == 4_000 && segs[0].endMs == 6_500 && segs[0].text == "hola")
    #expect(SpeechPlan.segments(span: span, tokens: [], text: "  ", confidence: 0.6).isEmpty)
}
