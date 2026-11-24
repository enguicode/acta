import ActaMacProtocol
import CoreGraphics
import Foundation
import ImageIO
import Testing
@testable import ActaMacCore

private func gradient(reversed: Bool = false, width: Int = 64, height: Int = 48) -> CGImage {
    var px = [UInt8](repeating: 0, count: width * height)
    for y in 0..<height {
        for x in 0..<width {
            let v = UInt8(x * 255 / (width - 1))
            px[y * width + x] = reversed ? 255 - v : v
        }
    }
    let provider = CGDataProvider(data: Data(px) as CFData)!
    return CGImage(
        width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 8, bytesPerRow: width,
        space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue),
        provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
    )!
}

@Test func `dhash of gray thumbnail sets bits where left is brighter`() {
    var px = [UInt8](repeating: 0, count: 72)
    for y in 0..<8 { for x in 0..<9 { px[y * 9 + x] = UInt8(200 - x * 10) } } // decreciente
    #expect(DHash.hash(gray9x8: px) == UInt64.max)
    #expect(DHash.hex(0xFF) == "00000000000000ff")
    #expect(DHash.distance(0b1010, 0b0110) == 2)
}

@Test func `dhash separates opposite images and matches identical ones`() throws {
    let a = try #require(DHash.hash(gradient()))
    let b = try #require(DHash.hash(gradient()))
    let c = try #require(DHash.hash(gradient(reversed: true)))
    #expect(DHash.distance(a, b) == 0)
    #expect(DHash.distance(a, c) > 40)
}

@Test func `slide change needs stability and a big enough difference`() {
    var d = SlideChangeDetector(threshold: 10, stableFrames: 2)
    let slide1: UInt64 = 0x0F0F_0F0F_0F0F_0F0F
    let slide2: UInt64 = 0xF0F0_F0F0_F0F0_F0F0
    do { let r = d.observe(slide1); #expect(!r, "first frame not yet stable") }
    do { let r = d.observe(slide1); #expect(r, "stable twice → first slide saved") }
    do { let r = d.observe(slide1); #expect(!r, "same slide never saved again") }
    do { let r = d.observe(slide1 ^ 0b11); #expect(!r, "tiny noise is not a new slide") }
    do { let r = d.observe(slide2); #expect(!r, "transition frame") }
    do { let r = d.observe(slide2); #expect(r, "new slide once stable") }
    d.reset()
    do { let r = d.observe(slide2); #expect(!r) }
    do { let r = d.observe(slide2); #expect(r, "after reset the current slide counts again") }
}

@Test func `frame ring keeps the window and finds the nearest frame`() {
    var r = FrameRing(seconds: 20)
    for t in stride(from: Int64(0), through: 30_000, by: 1000) {
        r.append(RingFrame(atUnixMs: t, jpeg: Data([1]), phash: UInt64(t), width: 1, height: 1))
    }
    #expect(r.frames.first?.atUnixMs == 10_000)
    #expect(r.nearest(14_400)?.atUnixMs == 14_000)
    #expect(r.nearest(31_000)?.atUnixMs == 30_000)
    #expect(r.nearest(-60_000) == nil)
}

@Test func `capture file names`() {
    #expect(CaptureFiles.name(atUnixMs: 1_700_000_000_123, reason: .slideChange) == "1700000000123-slide_change.jpg")
    #expect(CaptureFiles.name(atUnixMs: 5, reason: .candidate) == "5.jpg")
}

@Test func `capture write is atomic and returns the event`() throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("acta-test-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: dir) }
    let jpeg = try #require(Jpeg.encode(gradient(), quality: 0.8))
    let frame = RingFrame(atUnixMs: 42, jpeg: jpeg, phash: 0xAB, width: 64, height: 48)
    let ev = try CaptureFiles.write(frame, reason: .keyword, dir: dir.path, keyword: "deadline")
    #expect(ev.path.hasSuffix("/42-keyword.jpg"))
    #expect(ev.phash == "00000000000000ab")
    #expect(ev.keyword == "deadline")
    #expect(FileManager.default.fileExists(atPath: ev.path))
    #expect(!FileManager.default.fileExists(atPath: ev.path + ".tmp"))
    let again = try CaptureFiles.write(frame, reason: .keyword, dir: dir.path, keyword: nil)
    #expect(again.path != ev.path, "never overwrites an existing capture")
}

// MARK: - Seguir la ventana entre pantallas (hasta 5 monitores, escalas distintas)

private func win(_ id: UInt32, _ title: String, _ w: Double, _ h: Double, scale: Double = 2) -> WindowCandidate {
    WindowCandidate(id: id, title: title, width: w, height: h, scale: scale)
}

@Test func `the shared content window wins over the bigger main Teams window`() {
    let wins = [
        win(1, "Chat | Microsoft Teams", 1600, 1000),
        win(2, "Daily del equipo | Microsoft Teams", 1200, 800),
        win(3, "Contenido compartido de Ana | Microsoft Teams", 1000, 700),
    ]
    #expect(WindowTracking.pick(wins, titleHint: "Daily del equipo")?.id == 3)
    #expect(WindowTracking.pick(Array(wins.prefix(2)), titleHint: "Daily del equipo")?.id == 2)
    #expect(WindowTracking.pick(Array(wins.prefix(2)), titleHint: nil)?.id == 1, "sin pista, la más grande")
    #expect(WindowTracking.pick([win(9, "x", 100, 80)], titleHint: nil) == nil, "demasiado pequeña")
}

@Test func `moving to a display with another scale or resizing reconfigures the stream`() {
    let a = WindowGeometry(id: 2, width: 1200, height: 800, scale: 2)
    #expect(WindowTracking.needsUpdate(current: nil, new: a))
    #expect(!WindowTracking.needsUpdate(current: a, new: a))
    #expect(!WindowTracking.needsUpdate(current: a, new: WindowGeometry(id: 2, width: 1210, height: 805, scale: 2)), "ruido de unos px")
    #expect(WindowTracking.needsUpdate(current: a, new: WindowGeometry(id: 2, width: 1200, height: 800, scale: 1)), "pantalla no Retina")
    #expect(WindowTracking.needsUpdate(current: a, new: WindowGeometry(id: 2, width: 1600, height: 1000, scale: 2)), "redimensionada")
    #expect(WindowTracking.needsUpdate(current: a, new: WindowGeometry(id: 3, width: 1200, height: 800, scale: 2)), "otra ventana")
}

@Test func `stream size keeps the aspect ratio under max width`() {
    let (w, h) = WindowTracking.outputSize(WindowGeometry(id: 1, width: 1440, height: 900, scale: 2), maxWidth: 1920)
    #expect(w == 1920 && h == 1200)
    let (w1, h1) = WindowTracking.outputSize(WindowGeometry(id: 1, width: 1000, height: 600, scale: 1), maxWidth: 1920)
    #expect(w1 == 1000 && h1 == 600)
}

// MARK: - Contenido estático (19-nov-2026: sin ninguna captura)

/// ScreenCaptureKit solo entrega frames `.complete` cuando cambia algo: con una
/// diapositiva quieta llega UNO y el detector, que pedía dos iguales, no
/// guardaba nunca. Un "sigue igual" cuenta como otro frame igual.
@Test func `a static slide is saved even if only one complete frame arrives`() {
    var d = SlideChangeDetector(threshold: 10, stableFrames: 2)
    do { let r = d.observeUnchanged(); #expect(!r, "nada que repetir todavía") }
    let slide: UInt64 = 0x0F0F_0F0F_0F0F_0F0F
    do { let r = d.observe(slide); #expect(!r) }
    do { let r = d.observeUnchanged(); #expect(r, "quieta un intervalo → guardada") }
    do { let r = d.observeUnchanged(); #expect(!r, "y solo una vez") }
}

@Test func `an unchanged frame is repeated once per frame interval without new frames`() {
    #expect(!StaticFrames.shouldRepeat(lastObservedMs: nil, nowMs: 10_000, fps: 1))
    #expect(!StaticFrames.shouldRepeat(lastObservedMs: 9_500, nowMs: 10_000, fps: 1), "aún dentro del intervalo")
    #expect(!StaticFrames.shouldRepeat(lastObservedMs: 8_900, nowMs: 10_000, fps: 1), "un frame que llega algo tarde no es «sin cambios»")
    #expect(StaticFrames.shouldRepeat(lastObservedMs: 8_400, nowMs: 10_000, fps: 1), "1,5 intervalos sin frames")
    #expect(StaticFrames.shouldRepeat(lastObservedMs: 9_200, nowMs: 10_000, fps: 2), "a 2 fps el intervalo es 500 ms")
}

// MARK: - Sin app: la pantalla principal

@Test func `without a meeting app the main display is captured`() {
    #expect(ScreenTarget(pid: 0) == .display)
    #expect(ScreenTarget(pid: -1) == .display)
    #expect(ScreenTarget(pid: 4242) == .window(pid: 4242))
}

@Test func `whole screen mode never saves automatic captures`() {
    #expect(!ScreenTarget.display.savesAutomaticCaptures(axSharing: nil))
    #expect(!ScreenTarget.display.savesAutomaticCaptures(axSharing: true), "ni aunque AX diga que comparte")
    #expect(ScreenTarget.window(pid: 42).savesAutomaticCaptures(axSharing: nil))
    #expect(ScreenTarget.window(pid: 42).savesAutomaticCaptures(axSharing: true))
    #expect(!ScreenTarget.window(pid: 42).savesAutomaticCaptures(axSharing: false), "AX dice que no comparte")
}

@Test func `acta itself is excluded from display captures`() {
    #expect(ScreenTarget.excludedPids(selfPid: 10, parentPid: 9) == [10, 9])
    #expect(ScreenTarget.excludedPids(selfPid: 10, parentPid: 1) == [10], "launchd no es acta")
}

// MARK: - Recortes del portapapeles

@Test func `only new non-private images on the clipboard are imported`() {
    #expect(ClipboardPolicy.imageType(in: ["public.png", "public.tiff"]) == "public.png")
    #expect(ClipboardPolicy.imageType(in: ["public.tiff"]) == "public.tiff")
    #expect(ClipboardPolicy.imageType(in: ["public.jpeg"]) == "public.jpeg")
    #expect(ClipboardPolicy.imageType(in: ["public.utf8-plain-text"]) == nil, "texto no")
    #expect(ClipboardPolicy.imageType(in: ["public.png", "org.nspasteboard.ConcealedType"]) == nil, "contraseñas y similares")
    #expect(ClipboardPolicy.imageType(in: ["public.png", "org.nspasteboard.TransientType"]) == nil)
    // Copiar un fichero en Finder trae su icono como TIFF: eso no es un recorte.
    #expect(ClipboardPolicy.imageType(in: ["public.file-url", "public.tiff"]) == nil)
    #expect(ClipboardPolicy.isNew(changeCount: 5, last: 4))
    #expect(!ClipboardPolicy.isNew(changeCount: 4, last: 4))
}

@Test func `a clipboard image is encoded as jpeg under max width`() throws {
    let png = NSMutableData()
    let dest = try #require(CGImageDestinationCreateWithData(png as CFMutableData, "public.png" as CFString, 1, nil))
    CGImageDestinationAddImage(dest, gradient(width: 400, height: 300), nil)
    #expect(CGImageDestinationFinalize(dest))
    let img = try #require(ClipboardImage.decode(png as Data, maxWidth: 200))
    #expect(img.width == 200 && img.height == 150)
    #expect(img.jpeg.starts(with: [0xFF, 0xD8]), "JPEG")
    let full = try #require(ClipboardImage.decode(png as Data, maxWidth: 1920))
    #expect(full.width == 400 && full.height == 300, "nunca se agranda")
    #expect(ClipboardImage.decode(Data([1, 2, 3]), maxWidth: 100) == nil)
}

// MARK: - dHash con diapositivas reales (PNG/JPEG de verdad)

import CoreText
import UniformTypeIdentifiers

private enum Slides {
    /// `template`: 0 = barra azul sobre blanco, 1 = fondo oscuro, 2 = barra verde sobre gris claro.
    /// `cursor`: posición del puntero. `video`: miniatura del presentador (192×108) en la esquina
    /// inferior izquierda, cada `video` distinto es otra imagen.
    static func render(template: Int = 0, title: String, bullets: [String], cursor: CGPoint? = nil, clock: String = "10:04", video: Int? = nil) -> CGImage {
        let w = 1280, h = 720
        let dark = template == 1
        let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
        let bg: CGColor = dark ? CGColor(red: 0.08, green: 0.1, blue: 0.2, alpha: 1)
            : template == 2 ? CGColor(red: 0.9, green: 0.9, blue: 0.9, alpha: 1) : CGColor(red: 1, green: 1, blue: 1, alpha: 1)
        let bar: CGColor = template == 2 ? CGColor(red: 0.1, green: 0.55, blue: 0.3, alpha: 1) : CGColor(red: 0.15, green: 0.35, blue: 0.8, alpha: 1)
        ctx.setFillColor(bg)
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        ctx.setFillColor(bar)
        ctx.fill(CGRect(x: 0, y: h - 110, width: w, height: 110)) // barra de título de la plantilla
        func text(_ s: String, x: CGFloat, y: CGFloat, size: CGFloat, white: Bool) {
            let font = CTFontCreateWithName("Helvetica" as CFString, size, nil)
            let color = white ? CGColor(red: 1, green: 1, blue: 1, alpha: 1) : CGColor(red: 0, green: 0, blue: 0, alpha: 1)
            let attrs: [CFString: Any] = [kCTFontAttributeName: font, kCTForegroundColorAttributeName: color]
            let line = CTLineCreateWithAttributedString(CFAttributedStringCreate(nil, s as CFString, attrs as CFDictionary))
            ctx.textPosition = CGPoint(x: x, y: y)
            CTLineDraw(line, ctx)
        }
        text(title, x: 60, y: CGFloat(h) - 75, size: 48, white: true)
        for (i, b) in bullets.enumerated() { text("• " + b, x: 80, y: CGFloat(h) - 200 - CGFloat(i) * 70, size: 34, white: dark) }
        text(clock, x: CGFloat(w) - 120, y: 20, size: 22, white: dark)
        if let video {
            var seed = UInt64(video) &* 0x9E37_79B9_7F4A_7C15 | 1
            for by in 0..<9 {
                for bx in 0..<16 {
                    seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17
                    let v = CGFloat(seed % 256) / 255
                    ctx.setFillColor(CGColor(red: v, green: CGFloat((seed >> 8) % 256) / 255, blue: CGFloat((seed >> 16) % 256) / 255, alpha: 1))
                    ctx.fill(CGRect(x: 20 + bx * 12, y: 20 + by * 12, width: 12, height: 12))
                }
            }
        }
        if let cursor {
            ctx.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 1))
            ctx.fill(CGRect(x: cursor.x, y: cursor.y, width: 14, height: 22))
        }
        return ctx.makeImage()!
    }

    static func jpeg(_ image: CGImage, quality: Double = 0.6) -> CGImage {
        let data = Jpeg.encode(image, quality: quality)!
        return CGImageSourceCreateImageAtIndex(CGImageSourceCreateWithData(data as CFData, nil)!, 0, nil)!
    }
}

/// Calibrado con diapositivas dibujadas a 1280×720 (CoreText). El dHash 9×8
/// solo ve el ruido (1-2 bits) y el texto denso (17), no el cambio de una
/// diapositiva con la misma plantilla y 3-4 viñetas (1-3 bits). La firma de
/// cuadrículas 32×18 + extensión espacial detecta todo y no se dispara con
/// el ruido (cursor, reloj, JPEG, miniatura de vídeo en una esquina).
@Test func `thumbnail signature detects every slide change and no noise`() throws {
    let b1 = ["Lanzar la app en iOS", "Migrar a Kubernetes", "Contratar dos personas"]
    let b2 = ["Revisar el presupuesto", "Cerrar la ronda", "Definir el plan de ventas", "Preparar el informe"]
    let base = Slides.render(title: "Roadmap 2027", bullets: b1)
    let long1 = ["Migración completa de los servicios legacy a contenedores durante el segundo trimestre", "Revisión de costes cloud con el equipo de finanzas y dirección técnica", "Plan de formación en Kubernetes para todo el equipo de plataforma", "Auditoría de seguridad externa y corrección de hallazgos críticos", "Nuevo proceso de guardias con rotación semanal y compensación", "Observabilidad unificada con Grafana, Prometheus y trazas distribuidas"]
    let long2 = ["Objetivos comerciales del próximo año fiscal por región y por canal de venta", "Contratación de diez perfiles senior en ingeniería, producto y diseño", "Lanzamiento del plan empresarial con facturación anual y soporte dedicado", "Alianzas estratégicas con tres integradores en Latinoamérica y Europa", "Reducción del tiempo de incorporación de clientes de cuatro semanas a una", "Programa de fidelización con descuentos escalonados por volumen de uso"]

    let changes: [(String, CGImage, CGImage)] = [
        ("denso", Slides.render(title: "Plataforma 2027", bullets: long1), Slides.render(title: "Negocio 2027", bullets: long2)),
        ("disperso, misma plantilla", base, Slides.render(title: "Presupuesto", bullets: b2)),
        ("disperso, 3 viñetas distintas", base, Slides.render(title: "Roadmap 2027", bullets: ["Abrir oficina en Madrid", "Cerrar tres contratos", "Pasar la auditoría"])),
        ("solo el título", base, Slides.render(title: "Resumen 2027", bullets: b1)),
        ("fondo oscuro", base, Slides.render(template: 1, title: "Roadmap 2027", bullets: b1)),
        ("otra plantilla, mismo texto", base, Slides.render(template: 2, title: "Roadmap 2027", bullets: b1)),
    ]
    let noise: [(String, CGImage, CGImage)] = [
        ("cursor + reloj", base, Slides.render(title: "Roadmap 2027", bullets: b1, cursor: CGPoint(x: 700, y: 300), clock: "10:05")),
        ("cursor movido", Slides.render(title: "Roadmap 2027", bullets: b1, cursor: CGPoint(x: 200, y: 500)), Slides.render(title: "Roadmap 2027", bullets: b1, cursor: CGPoint(x: 900, y: 150))),
        ("reloj", base, Slides.render(title: "Roadmap 2027", bullets: b1, clock: "10:05")),
        ("JPEG 0.6", base, Slides.jpeg(base)),
        ("JPEG 0.3 sobre ruido", Slides.render(title: "Roadmap 2027", bullets: b1, cursor: CGPoint(x: 400, y: 300)), Slides.jpeg(base, quality: 0.3)),
        ("vídeo del presentador cambia", Slides.render(title: "Roadmap 2027", bullets: b1, video: 1), Slides.render(title: "Roadmap 2027", bullets: b1, video: 2)),
        ("vídeo + cursor + reloj", Slides.render(title: "Roadmap 2027", bullets: b1, video: 1), Slides.render(title: "Roadmap 2027", bullets: b1, cursor: CGPoint(x: 900, y: 450), clock: "10:06", video: 3)),
    ]
    for (name, a, b) in changes {
        let d = SlideThumb.compare(try #require(SlideThumb.make(a)), try #require(SlideThumb.make(b)))
        #expect(d.isSlideChange, "\(name) debe detectarse")
    }
    for (name, a, b) in noise {
        let d = SlideThumb.compare(try #require(SlideThumb.make(a)), try #require(SlideThumb.make(b)))
        #expect(!d.isSlideChange, "\(name) es ruido")
    }
}

@Test func `detector combines dhash and thumbnail so sparse slides are saved`() throws {
    let b1 = ["Lanzar la app en iOS", "Migrar a Kubernetes", "Contratar dos personas"]
    let s1 = Slides.render(title: "Roadmap 2027", bullets: b1)
    let s2 = Slides.render(title: "Presupuesto", bullets: ["Revisar el presupuesto", "Cerrar la ronda", "Definir el plan de ventas", "Preparar el informe"])
    let noisy = Slides.render(title: "Roadmap 2027", bullets: b1, cursor: CGPoint(x: 700, y: 300), clock: "10:05")
    func obs(_ d: inout SlideChangeDetector, _ i: CGImage) -> Bool {
        d.observe(DHash.hash(i)!, thumb: SlideThumb.make(i))
    }
    var d = SlideChangeDetector(threshold: 10, stableFrames: 2)
    #expect(!obs(&d, s1))
    #expect(obs(&d, s1), "first slide")
    #expect(!obs(&d, noisy), "cursor + clock")
    #expect(!obs(&d, noisy))
    #expect(!obs(&d, s2), "transition")
    #expect(obs(&d, s2), "sparse slide on the same template is saved")
    #expect(!obs(&d, s2))
}

@Test func `thumbnail hex round trips`() throws {
    let t = try #require(SlideThumb.make(gradient()))
    #expect(t.count == SlideThumb.cols * SlideThumb.rows)
    #expect(SlideThumb.parse(SlideThumb.hex(t)) == t)
    #expect(SlideThumb.parse("zz") == nil)
}

/// dHash a secas: el ruido queda en 1-2 bits y el texto denso en 17; el
/// disperso (1-3 bits) NO lo ve (por eso existe `SlideThumb`).
@Test func `dhash alone misses sparse slides but sees dense ones`() throws {
    let bullets = ["Lanzar la app en iOS", "Migrar a Kubernetes", "Contratar dos personas"]
    let a = Slides.render(title: "Roadmap 2027", bullets: bullets)
    let aNoisy = Slides.render(title: "Roadmap 2027", bullets: bullets, cursor: CGPoint(x: 700, y: 300), clock: "10:05")
    let aJpeg = Slides.jpeg(a)
    let long1 = ["Migración completa de los servicios legacy a contenedores durante el segundo trimestre", "Revisión de costes cloud con el equipo de finanzas y dirección técnica", "Plan de formación en Kubernetes para todo el equipo de plataforma", "Auditoría de seguridad externa y corrección de hallazgos críticos", "Nuevo proceso de guardias con rotación semanal y compensación", "Observabilidad unificada con Grafana, Prometheus y trazas distribuidas"]
    let long2 = ["Objetivos comerciales del próximo año fiscal por región y por canal de venta", "Contratación de diez perfiles senior en ingeniería, producto y diseño", "Lanzamiento del plan empresarial con facturación anual y soporte dedicado", "Alianzas estratégicas con tres integradores en Latinoamérica y Europa", "Reducción del tiempo de incorporación de clientes de cuatro semanas a una", "Programa de fidelización con descuentos escalonados por volumen de uso"]
    let h = try [a, aNoisy, aJpeg, Slides.render(title: "Plataforma 2027", bullets: long1), Slides.render(title: "Negocio 2027", bullets: long2)]
        .map { try #require(DHash.hash($0)) }
    let noise = SlideChangeDetector(threshold: 10, stableFrames: 2).noise
    #expect(DHash.distance(h[0], h[1]) <= noise, "cursor + reloj no es otra diapositiva")
    #expect(DHash.distance(h[0], h[2]) <= noise, "recompresión JPEG no es otra diapositiva")
    #expect(DHash.distance(h[3], h[4]) >= 10, "texto denso distinto sí es otra diapositiva")
}
