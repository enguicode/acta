// screen.* — capturas de la pantalla compartida (opt-in, docs §8).
//
//   SCStream (ventana de la reunión, ≈1 fps, ancho ≤ max_width)
//     → JPEG 0.8 + dHash 64 bits → anillo de `ring_seconds`
//     → cambio de diapositiva: distancia ≥ phash_threshold respecto a la
//       última guardada y estable `stable_frames` frames → captures/
//     → candidata cada `candidate_every_s` mientras se comparte → candidates/
//   ¿Comparte alguien? AX ("X está presentando") y, si AX no dice nada,
//   el propio detector de diapositivas (source "frame").
//   screen.snapshot devuelve el frame más cercano del anillo (sin evento).
//   Sin app (pid 0, grabación manual): la pantalla principal sin acta.
//   SCK solo entrega frames cuando cambia algo: si no llega ninguno en 1,5
//   intervalos, el último cuenta como "sigue igual" (si no, una diapositiva
//   quieta no se guardaba nunca).

import ActaMacProtocol
import CoreGraphics
import CoreImage
import CoreMedia
import Foundation
import ImageIO
@preconcurrency import ScreenCaptureKit
import UniformTypeIdentifiers

// MARK: - Lógica pura

enum DHash {
    /// dHash 64 bits: miniatura gris 9×8, bit = píxel izquierdo > derecho.
    static func hash(_ image: CGImage) -> UInt64? {
        var px = [UInt8](repeating: 0, count: 9 * 8)
        let ok: Bool = px.withUnsafeMutableBytes { buf in
            guard let ctx = CGContext(
                data: buf.baseAddress, width: 9, height: 8, bitsPerComponent: 8, bytesPerRow: 9,
                space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue
            ) else { return false }
            ctx.interpolationQuality = .medium
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: 9, height: 8))
            return true
        }
        return ok ? hash(gray9x8: px) : nil
    }

    static func hash(gray9x8 px: [UInt8]) -> UInt64 {
        precondition(px.count == 72)
        var h: UInt64 = 0
        for y in 0..<8 {
            for x in 0..<8 {
                h <<= 1
                if px[y * 9 + x] > px[y * 9 + x + 1] { h |= 1 }
            }
        }
        return h
    }

    static func distance(_ a: UInt64, _ b: UInt64) -> Int { (a ^ b).nonzeroBitCount }

    static func hex(_ h: UInt64) -> String { String(format: "%016llx", h) }
}

/// Firma de contenido: miniatura gris 64×36.
/// El dHash 9×8 no distingue dos diapositivas con la misma plantilla y poco
/// texto (1-3 bits, igual que el ruido); aquí se mira DÓNDE cambia la imagen:
/// un cambio de diapositiva toca muchas baldosas repartidas por la pantalla,
/// el ruido (cursor, reloj, miniatura de vídeo en una esquina) solo unas pocas
/// juntas. Mismo algoritmo en Rust (`acta-audio` `supervisor/gate.rs`).
enum SlideThumb {
    static let cols = 64
    static let rows = 36
    /// Salto de gris (0-255) de una celda que cuenta como "cambió"
    /// (suavizado y JPEG mueven 1-6).
    static let cellDelta = 10
    /// Un grupo de celdas cambiadas contiguas es una "superposición" (puntero,
    /// miniatura de vídeo, imagen que se mueve) si cabe en una esquina
    /// (≤ 25 % del ancho y del alto) y está casi relleno: no cuenta.
    /// El texto que cambia es disperso: celdas sueltas dentro de su caja.
    static let overlayMaxCols = 16
    static let overlayMaxRows = 9
    static let overlayFill = 0.5
    /// Celdas cambiadas (fuera de superposiciones) y columnas distintas mínimas.
    static let minCells = 6
    static let minColumns = 5
    /// Cambio global de luminosidad media que basta solo (fondo/plantilla).
    static let meanDelta = 12.0

    struct Diff: Equatable {
        /// Todas las celdas que cambiaron.
        var changedCells: Int
        /// Las que quedan sin las superposiciones.
        var cells: Int
        var columns: Int
        var meanDelta: Double

        var isSlideChange: Bool {
            if meanDelta >= SlideThumb.meanDelta { return true }
            return cells >= SlideThumb.minCells && columns >= SlideThumb.minColumns
        }
    }

    /// 2304 bytes: se dibuja a 256×144 y se promedian bloques de 4×4.
    static func make(_ image: CGImage) -> [UInt8]? {
        let w = cols * 4, h = rows * 4
        var px = [UInt8](repeating: 0, count: w * h)
        let ok: Bool = px.withUnsafeMutableBytes { buf in
            guard let ctx = CGContext(
                data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
                space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue
            ) else { return false }
            ctx.interpolationQuality = .high
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        guard ok else { return nil }
        var out = [UInt8](repeating: 0, count: cols * rows)
        for ty in 0..<rows {
            for tx in 0..<cols {
                var sum = 0
                for y in 0..<4 { for x in 0..<4 { sum += Int(px[(ty * 4 + y) * w + tx * 4 + x]) } }
                out[ty * cols + tx] = UInt8(sum / 16)
            }
        }
        return out
    }

    static func compare(_ a: [UInt8], _ b: [UInt8], cellDelta: Int = SlideThumb.cellDelta) -> Diff {
        guard a.count == cols * rows, b.count == a.count else { return Diff(changedCells: 0, cells: 0, columns: 0, meanDelta: 0) }
        var changed = [Bool](repeating: false, count: a.count)
        var total = 0, sumA = 0, sumB = 0
        for i in 0..<a.count {
            sumA += Int(a[i]); sumB += Int(b[i])
            if abs(Int(a[i]) - Int(b[i])) >= cellDelta { changed[i] = true; total += 1 }
        }
        // Grupos de celdas contiguas (8 vecinas): las superposiciones se descartan.
        var seen = [Bool](repeating: false, count: a.count)
        var kept = 0
        var colSet: UInt64 = 0
        for start in 0..<a.count where changed[start] && !seen[start] {
            var stack = [start]
            seen[start] = true
            var members: [Int] = []
            var minX = cols, maxX = 0, minY = rows, maxY = 0
            while let i = stack.popLast() {
                members.append(i)
                let x = i % cols, y = i / cols
                minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
                for dy in -1...1 {
                    for dx in -1...1 {
                        let nx = x + dx, ny = y + dy
                        guard nx >= 0, nx < cols, ny >= 0, ny < rows else { continue }
                        let j = ny * cols + nx
                        if changed[j], !seen[j] { seen[j] = true; stack.append(j) }
                    }
                }
            }
            let w = maxX - minX + 1, h = maxY - minY + 1
            let fill = Double(members.count) / Double(w * h)
            if w <= overlayMaxCols, h <= overlayMaxRows, fill >= overlayFill { continue }
            kept += members.count
            for i in members { colSet |= 1 << UInt64(i % cols) }
        }
        return Diff(
            changedCells: total, cells: kept, columns: colSet.nonzeroBitCount,
            meanDelta: abs(Double(sumA - sumB)) / Double(a.count)
        )
    }

    static func hex(_ t: [UInt8]) -> String { t.map { String(format: "%02x", $0) }.joined() }

    static func parse(_ s: String) -> [UInt8]? {
        guard s.utf8.count == cols * rows * 2 else { return nil }
        var out: [UInt8] = []
        out.reserveCapacity(cols * rows)
        var it = s.utf8.makeIterator()
        while let hi = it.next(), let lo = it.next() {
            guard let v = UInt8(String(decoding: [hi, lo], as: UTF8.self), radix: 16) else { return nil }
            out.append(v)
        }
        return out
    }
}

/// Decide cuándo un frame es una diapositiva nueva.
struct SlideChangeDetector {
    let threshold: Int
    let stableFrames: Int
    /// Ruido permitido entre frames "iguales" (compresión, cursor, vídeo pequeño).
    var noise = 3
    private(set) var lastSaved: UInt64?
    private var lastSavedThumb: [UInt8]?
    private var previous: UInt64?
    private var previousThumb: [UInt8]?
    private var stableRun = 0

    init(threshold: Int, stableFrames: Int) {
        self.threshold = max(1, threshold)
        self.stableFrames = max(1, stableFrames)
    }

    /// `true` = guardar este frame como `slide_change`.
    mutating func observe(_ h: UInt64, thumb: [UInt8]? = nil) -> Bool {
        if let p = previous, !differs(p, previousThumb, h, thumb, limit: noise + 1) {
            stableRun += 1
        } else {
            stableRun = 1
        }
        previous = h
        previousThumb = thumb
        guard stableRun >= stableFrames else { return false }
        if let last = lastSaved, !differs(last, lastSavedThumb, h, thumb, limit: threshold) { return false }
        lastSaved = h
        lastSavedThumb = thumb
        return true
    }

    /// dHash a `limit` bits o más, o firma de baldosas con cambio extendido.
    private func differs(_ a: UInt64, _ ta: [UInt8]?, _ b: UInt64, _ tb: [UInt8]?, limit: Int) -> Bool {
        if DHash.distance(a, b) >= limit { return true }
        if let ta, let tb { return SlideThumb.compare(ta, tb).isSlideChange }
        return false
    }

    /// No llegó frame nuevo en un intervalo: el contenido sigue igual que el
    /// último (ScreenCaptureKit no repite frames sin cambios).
    mutating func observeUnchanged() -> Bool {
        guard let p = previous else { return false }
        return observe(p, thumb: previousThumb)
    }

    mutating func reset() {
        lastSaved = nil
        lastSavedThumb = nil
        previous = nil
        previousThumb = nil
        stableRun = 0
    }
}

enum StaticFrames {
    /// ¿Han pasado 1,5 intervalos sin frames? (margen para no confundir un
    /// frame que llega tarde con contenido quieto).
    static func shouldRepeat(lastObservedMs: Int64?, nowMs: Int64, fps: Double) -> Bool {
        guard let last = lastObservedMs else { return false }
        let interval = 1000 / max(0.1, fps)
        return Double(nowMs - last) >= interval * 1.5
    }
}

/// Qué se captura.
enum ScreenTarget: Equatable, Sendable {
    /// Ventana de la app de la reunión.
    case window(pid: Int32)
    /// Sin app detectada: la pantalla principal, sin las ventanas de acta.
    case display

    init(pid: Int32) { self = pid > 0 ? .window(pid: pid) : .display }

    /// ¿Se guardan capturas automáticas (cambio de diapositiva, candidatas)?
    /// Pantalla entera: nunca, no hay "alguien comparte" (solo el anillo para
    /// botón/palabra clave). Ventana: salvo que AX diga que no se comparte.
    func savesAutomaticCaptures(axSharing: Bool?) -> Bool {
        if case .display = self { return false }
        return axSharing != false
    }

    /// acta-mac y la app que lo lanzó (acta.app); launchd (1) no.
    static func excludedPids(selfPid: Int32, parentPid: Int32) -> [Int32] {
        parentPid > 1 ? [selfPid, parentPid] : [selfPid]
    }
}

enum ScreenErrors {
    /// TCC denegado (SCStreamError.userDeclined, -3801).
    static func isPermission(_ error: Error) -> Bool {
        let e = error as NSError
        return e.domain == SCStreamErrorDomain && e.code == SCStreamError.Code.userDeclined.rawValue
    }
}

struct RingFrame: Sendable {
    var atUnixMs: Int64
    var jpeg: Data
    var phash: UInt64
    var width: Int
    var height: Int
    var thumb: [UInt8]? = nil
}

/// Últimos N segundos de frames (ya en JPEG para no guardar bitmaps).
struct FrameRing {
    let seconds: Int
    private(set) var frames: [RingFrame] = []

    init(seconds: Int) { self.seconds = max(1, seconds) }

    mutating func append(_ f: RingFrame) {
        frames.append(f)
        let cutoff = f.atUnixMs - Int64(seconds) * 1000
        frames.removeAll { $0.atUnixMs < cutoff }
    }

    /// Frame más cercano a `at` dentro de la ventana (o el último si `at` es futuro cercano).
    func nearest(_ at: Int64, toleranceMs: Int64 = 5_000) -> RingFrame? {
        frames.min { abs($0.atUnixMs - at) < abs($1.atUnixMs - at) }
            .flatMap { abs($0.atUnixMs - at) <= Int64(seconds) * 1000 + toleranceMs ? $0 : nil }
    }

    var latest: RingFrame? { frames.last }
}

enum CaptureFiles {
    /// `<dir>/<unix_ms>-<reason>.jpg` (candidatas: `<dir>/<unix_ms>.jpg`).
    static func name(atUnixMs: Int64, reason: CaptureReasonName) -> String {
        reason == .candidate ? "\(atUnixMs).jpg" : "\(atUnixMs)-\(reason.rawValue).jpg"
    }

    static func write(_ frame: RingFrame, reason: CaptureReasonName, dir: String, keyword: String?) throws -> ScreenFrameEvent {
        var url = URL(fileURLWithPath: dir).appendingPathComponent(name(atUnixMs: frame.atUnixMs, reason: reason))
        var n = 1
        while FileManager.default.fileExists(atPath: url.path) {
            url = URL(fileURLWithPath: dir).appendingPathComponent("\(frame.atUnixMs)-\(reason.rawValue)-\(n).jpg")
            n += 1
        }
        try AtomicFile.write(frame.jpeg, to: url)
        return ScreenFrameEvent(
            path: url.path, atUnixMs: frame.atUnixMs, reason: reason, phash: DHash.hex(frame.phash),
            width: UInt32(frame.width), height: UInt32(frame.height), keyword: keyword,
            thumb: frame.thumb.map(SlideThumb.hex)
        )
    }
}

enum Jpeg {
    static func encode(_ image: CGImage, quality: Double) -> Data? {
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data as CFMutableData, UTType.jpeg.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(dest, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
        return CGImageDestinationFinalize(dest) ? data as Data : nil
    }
}

// MARK: - Seguir la ventana (lógica pura)

/// Una ventana capturable de la app de la reunión.
struct WindowCandidate: Equatable, Sendable {
    var id: UInt32
    var title: String
    var width: Double
    var height: Double
    /// Píxeles por punto de la pantalla donde está ahora.
    var scale: Double
}

struct WindowGeometry: Equatable, Sendable {
    var id: UInt32
    var width: Double
    var height: Double
    var scale: Double
}

/// Con varios monitores las ventanas se mueven: se vuelve a elegir la
/// ventana cada pocos segundos y, si cambia (otra ventana, otra pantalla con
/// otra escala o otro tamaño), se reconfigura el stream sin pararlo.
enum WindowTracking {
    /// Palabras de la ventana de contenido compartido de Teams (es/en).
    static let sharedHints = ["contenido compartido", "shared content", "compartiendo", "sharing", "presentando", "presenting"]

    static func pick(_ wins: [WindowCandidate], titleHint: String?) -> WindowCandidate? {
        let usable = wins.filter { $0.width > 200 && $0.height > 150 }
        let lower = { (w: WindowCandidate) in w.title.lowercased() }
        if let shared = usable.filter({ w in sharedHints.contains { lower(w).contains($0) } }).max(by: { $0.width * $0.height < $1.width * $1.height }) {
            return shared
        }
        if let hint = titleHint?.lowercased(), !hint.isEmpty,
           let w = usable.first(where: { lower($0).contains(hint) || (!lower($0).isEmpty && hint.contains(lower($0))) }) {
            return w
        }
        return usable.max { $0.width * $0.height < $1.width * $1.height }
    }

    static func needsUpdate(current: WindowGeometry?, new: WindowGeometry) -> Bool {
        guard let c = current else { return true }
        if c.id != new.id || c.scale != new.scale { return true }
        let dw = abs(c.width - new.width) / max(1, c.width)
        let dh = abs(c.height - new.height) / max(1, c.height)
        return dw > 0.05 || dh > 0.05
    }

    /// Tamaño de salida en píxeles reales, con el ancho ≤ `maxWidth`.
    static func outputSize(_ g: WindowGeometry, maxWidth: Int) -> (Int, Int) {
        let w = max(1, g.width * g.scale)
        let h = max(1, g.height * g.scale)
        let k = min(1, Double(maxWidth) / w)
        return (Int((w * k).rounded()), Int((h * k).rounded()))
    }
}

// MARK: - ScreenCaptureKit

final class ImageBox: @unchecked Sendable {
    let image: CGImage
    init(_ i: CGImage) { image = i }
}

final class WindowStreamer: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "acta.screen.frames")
    private let ci = CIContext(options: [.cacheIntermediates: false])
    private let onFrame: @Sendable (ImageBox) -> Void
    private let onStop: @Sendable (Error?) -> Void

    init(onFrame: @escaping @Sendable (ImageBox) -> Void, onStop: @escaping @Sendable (Error?) -> Void) {
        self.onFrame = onFrame
        self.onStop = onStop
    }

    private(set) var geometry: WindowGeometry?

    static func geometry(of window: SCWindow, filter: SCContentFilter) -> WindowGeometry {
        WindowGeometry(id: window.windowID, width: window.frame.width, height: window.frame.height, scale: Double(filter.pointPixelScale))
    }

    private static func configuration(_ g: WindowGeometry, maxWidth: Int, fps: Double) -> SCStreamConfiguration {
        let cfg = SCStreamConfiguration()
        let (w, h) = WindowTracking.outputSize(g, maxWidth: maxWidth)
        cfg.width = w
        cfg.height = h
        cfg.minimumFrameInterval = CMTime(seconds: 1 / max(0.1, fps), preferredTimescale: 600)
        cfg.pixelFormat = kCVPixelFormatType_32BGRA
        cfg.showsCursor = false
        cfg.queueDepth = 3
        return cfg
    }

    func start(window: SCWindow, maxWidth: Int, fps: Double) async throws {
        // `desktopIndependentWindow`: la ventana se captura esté en la pantalla que esté.
        let filter = SCContentFilter(desktopIndependentWindow: window)
        try await start(filter: filter, geometry: Self.geometry(of: window, filter: filter), maxWidth: maxWidth, fps: fps)
    }

    /// Pantalla principal sin las ventanas de acta (grabación sin app).
    func startDisplay(_ content: SCShareableContent, maxWidth: Int, fps: Double) async throws {
        guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first else {
            throw ProtocolError(code: .notFound, message: "no display to capture")
        }
        let excluded = ScreenTarget.excludedPids(selfPid: getpid(), parentPid: getppid())
        let apps = content.applications.filter { excluded.contains($0.processID) }
        let filter = SCContentFilter(display: display, excludingApplications: apps, exceptingWindows: [])
        let g = WindowGeometry(id: display.displayID, width: Double(display.width), height: Double(display.height), scale: Double(filter.pointPixelScale))
        try await start(filter: filter, geometry: g, maxWidth: maxWidth, fps: fps)
    }

    private func start(filter: SCContentFilter, geometry g: WindowGeometry, maxWidth: Int, fps: Double) async throws {
        let s = SCStream(filter: filter, configuration: Self.configuration(g, maxWidth: maxWidth, fps: fps), delegate: self)
        try s.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        try await s.startCapture()
        stream = s
        geometry = g
    }

    /// Otra ventana, otra pantalla (escala) u otro tamaño: se cambia en caliente.
    /// Devuelve `true` si hubo que reconfigurar.
    func follow(window: SCWindow, maxWidth: Int, fps: Double) async throws -> Bool {
        guard let s = stream else { return false }
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let g = Self.geometry(of: window, filter: filter)
        guard WindowTracking.needsUpdate(current: geometry, new: g) else { return false }
        if geometry?.id != g.id { try await s.updateContentFilter(filter) }
        try await s.updateConfiguration(Self.configuration(g, maxWidth: maxWidth, fps: fps))
        geometry = g
        return true
    }

    func stop() async {
        guard let s = stream else { return }
        stream = nil
        try? await s.stopCapture()
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sb.isValid,
              let atts = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let raw = atts.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let pb = CMSampleBufferGetImageBuffer(sb)
        else { return }
        let img = CIImage(cvPixelBuffer: pb)
        guard let cg = ci.createCGImage(img, from: img.extent) else { return }
        onFrame(ImageBox(cg))
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        onStop(error)
    }

    /// Ventana de la app: la de contenido compartido, si no la que encaja con
    /// el título, si no la más grande (`WindowTracking.pick`).
    static func findWindow(pid: Int32, titleHint: String?) async throws -> SCWindow? {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        let wins = content.windows.filter { $0.owningApplication?.processID == pid }
        let candidates = wins.map {
            WindowCandidate(id: $0.windowID, title: $0.title ?? "", width: $0.frame.width, height: $0.frame.height, scale: 1)
        }
        guard let best = WindowTracking.pick(candidates, titleHint: titleHint) else { return nil }
        return wins.first { $0.windowID == best.id }
    }
}

// MARK: - Servicio

public actor ScreenService {
    private let output: Output
    private var args: ScreenStartArgs?
    private var streamer: WindowStreamer?
    private var ring = FrameRing(seconds: 20)
    private var detector = SlideChangeDetector(threshold: 10, stableFrames: 2)
    private var sharing = false
    private var axSharing: Bool?
    private var axEverSaw = false
    private var lastCandidateMs: Int64 = 0
    private var axTask: Task<Void, Never>?
    private var restartTask: Task<Void, Never>?
    private var staticTask: Task<Void, Never>?
    /// Último frame real o repetido que vio el detector.
    private var lastObservedMs: Int64?

    public init(output: Output) { self.output = output }

    private var followTask: Task<Void, Never>?

    public func start(_ a: ScreenStartArgs) async throws {
        await stop()
        // No se mira `CGPreflightScreenCaptureAccess()`: su respuesta es la del
        // momento en que arrancó el proceso, y tras conceder el permiso en
        // Ajustes seguía diciendo que no (17-nov-2026). ScreenCaptureKit
        // pregunta a TCC en cada llamada.
        guard a.format == "jpeg" else { throw ProtocolError(code: .invalidArgs, message: "only jpeg is supported") }
        try FileManager.default.createDirectory(atPath: a.outDir, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(atPath: a.candidatesDir, withIntermediateDirectories: true)
        args = a
        ring = FrameRing(seconds: Int(a.ringSeconds))
        detector = SlideChangeDetector(threshold: Int(a.phashThreshold), stableFrames: Int(a.stableFrames))
        sharing = false
        axSharing = nil
        axEverSaw = false
        lastObservedMs = nil
        do {
            try await openStream()
        } catch {
            // Rust reintenta con otro screen.start; no quedar «activo» a medias.
            args = nil
            throw error
        }
        startStaticTicker()
        if case .window(let pid) = ScreenTarget(pid: a.pid) { startAxPolling(pid: pid) }
    }

    private func openStream() async throws {
        guard let a = args else { return }
        let target = ScreenTarget(pid: a.pid)
        let s = WindowStreamer(
            onFrame: { [weak self] box in
                let at = Clock.unixMs()
                Task { await self?.handle(box, at: at) }
            },
            onStop: { [weak self] err in
                Task { await self?.streamStopped(err) }
            }
        )
        do {
            switch target {
            case .window(let pid):
                let found: SCWindow?
                do {
                    found = try await WindowStreamer.findWindow(pid: pid, titleHint: a.windowTitleHint)
                } catch {
                    throw ProtocolError(code: .permissionDenied, message: "screen recording permission not granted: \(error.localizedDescription)")
                }
                guard let window = found else {
                    throw ProtocolError(code: .notFound, message: "no capturable window for pid \(pid)")
                }
                try await s.start(window: window, maxWidth: Int(a.maxWidth), fps: Double(a.fps))
            case .display:
                let content: SCShareableContent
                do {
                    content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                } catch {
                    throw ProtocolError(code: .permissionDenied, message: "screen recording permission not granted: \(error.localizedDescription)")
                }
                try await s.startDisplay(content, maxWidth: Int(a.maxWidth), fps: Double(a.fps))
            }
        } catch let e as ProtocolError {
            throw e
        } catch where ScreenErrors.isPermission(error) {
            throw ProtocolError(code: .permissionDenied, message: "screen recording permission not granted: \(error.localizedDescription)")
        } catch {
            throw ProtocolError(code: .internalError, message: "screen capture failed to start: \(error.localizedDescription)")
        }
        streamer = s
        if case .window = target { startFollowing() }
    }

    /// Cada 3 s: ¿sigue siendo la ventana buena, en la misma pantalla y tamaño?
    private func startFollowing() {
        followTask?.cancel()
        followTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(3))
                await self?.followWindow()
            }
        }
    }

    private func followWindow() async {
        guard let a = args, let s = streamer else { return }
        do {
            guard let w = try await WindowStreamer.findWindow(pid: a.pid, titleHint: a.windowTitleHint) else { return }
            if try await s.follow(window: w, maxWidth: Int(a.maxWidth), fps: Double(a.fps)) {
                await output.log("info", "screen: ventana \(w.windowID) \(Int(w.frame.width))x\(Int(w.frame.height)) (otra pantalla, tamaño o ventana)")
            }
        } catch {
            await output.log("warn", "screen: no se pudo seguir la ventana: \(error)")
        }
    }

    private func streamStopped(_ error: Error?) async {
        guard args != nil else { return }
        await output.log("warn", "screen stream stopped: \(error.map { "\($0)" } ?? "?"); retrying")
        streamer = nil
        restartTask?.cancel()
        restartTask = Task { [weak self] in
            for _ in 0..<60 {
                try? await Task.sleep(for: .seconds(5))
                guard let self, await self.isActive, await !self.hasStream else { return }
                if (try? await self.openStream()) != nil { return }
            }
        }
    }

    var isActive: Bool { args != nil }
    var hasStream: Bool { streamer != nil }

    private func startAxPolling(pid: Int32) {
        axTask?.cancel()
        axTask = Task.detached(priority: .utility) { [weak self] in
            while !Task.isCancelled {
                if AXSnapshot.isTrusted() {
                    let r = TeamsAX.readLive(pid: pid)
                    await self?.axUpdate(sharing: r.isMeeting ? (r.sharing ?? false) : nil, presenter: r.presenter)
                }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    /// AX solo es fiable para "no comparte" si alguna vez vio "comparte" (si
    /// las heurísticas no encajan nunca, manda el detector de frames).
    /// Que comparta el propio usuario (presenter "") queda fuera del MVP: cuenta como no.
    private func axUpdate(sharing raw: Bool?, presenter: String?) async {
        guard args != nil else { return }
        let ax: Bool? = presenter == "" ? false : raw
        if ax == true { axEverSaw = true }
        axSharing = axEverSaw ? (ax ?? false) : nil
        guard let now = axSharing, now != sharing else { return }
        sharing = now
        if !now { detector.reset() }
        await output.emit(.screenShareState(ScreenShareStateEvent(sharing: now, presenter: now ? presenter : nil, source: "ax")))
    }

    /// Cada segundo: si SCK no ha mandado nada en 1,5 intervalos, el último
    /// frame sigue en pantalla.
    private func startStaticTicker() {
        staticTask?.cancel()
        staticTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                await self?.repeatIfStatic()
            }
        }
    }

    private func repeatIfStatic() async {
        guard let a = args, streamer != nil, var f = ring.latest else { return }
        let now = Clock.unixMs()
        guard StaticFrames.shouldRepeat(lastObservedMs: lastObservedMs, nowMs: now, fps: Double(a.fps)) else { return }
        lastObservedMs = now
        // Se reanilla con la hora actual: el snapshot (botón, palabra clave)
        // encuentra la diapositiva quieta aunque lleve minutos sin cambiar.
        f.atUnixMs = now
        ring.append(f)
        await evaluate(f, changed: false, lowJpeg: nil)
    }

    private func handle(_ box: ImageBox, at: Int64) async {
        guard args != nil else { return }
        guard let jpeg = Jpeg.encode(box.image, quality: 0.8), let h = DHash.hash(box.image) else { return }
        let frame = RingFrame(atUnixMs: at, jpeg: jpeg, phash: h, width: box.image.width, height: box.image.height, thumb: SlideThumb.make(box.image))
        ring.append(frame)
        lastObservedMs = at
        await evaluate(frame, changed: true, lowJpeg: Jpeg.encode(box.image, quality: 0.6))
    }

    /// `changed = false`: el frame es el último repetido (contenido quieto).
    private func evaluate(_ frame: RingFrame, changed: Bool, lowJpeg: Data?) async {
        guard let a = args else { return }
        let at = frame.atUnixMs
        // Pantalla entera (sin app): no hay "alguien comparte", así que no se
        // guardan capturas automáticas (solo el anillo para botón/palabra clave).
        // Con AX diciendo "no comparte" tampoco se buscan diapositivas.
        guard ScreenTarget(pid: a.pid).savesAutomaticCaptures(axSharing: axSharing) else { return }
        if changed ? detector.observe(frame.phash, thumb: frame.thumb) : detector.observeUnchanged() {
            if !sharing, axSharing == nil {
                sharing = true
                await output.emit(.screenShareState(ScreenShareStateEvent(sharing: true, presenter: nil, source: "frame")))
            }
            await save(frame, reason: .slideChange, dir: a.outDir, keyword: nil, emit: true)
        }
        if sharing, at - lastCandidateMs >= Int64(a.candidateEveryS) * 1000 {
            lastCandidateMs = at
            var c = frame
            if let lowJpeg { c.jpeg = lowJpeg }
            await save(c, reason: .candidate, dir: a.candidatesDir, keyword: nil, emit: true)
        }
    }

    @discardableResult
    private func save(_ f: RingFrame, reason: CaptureReasonName, dir: String, keyword: String?, emit: Bool) async -> ScreenFrameEvent? {
        do {
            let ev = try CaptureFiles.write(f, reason: reason, dir: dir, keyword: keyword)
            if emit { await output.emit(.screenFrame(ev)) }
            return ev
        } catch {
            await output.log("error", "capture write failed: \(error)")
            return nil
        }
    }

    public func snapshot(_ s: ScreenSnapshotArgs) async throws -> ScreenSnapshotResult {
        guard let a = args else { throw ProtocolError(code: .notFound, message: "screen capture not running") }
        guard let f = ring.nearest(s.atUnixMs) else { return ScreenSnapshotResult(frame: nil) }
        let dir = s.reason == .candidate ? a.candidatesDir : a.outDir
        return ScreenSnapshotResult(frame: await save(f, reason: s.reason, dir: dir, keyword: s.keyword, emit: false))
    }

    public func stop() async {
        args = nil
        axTask?.cancel()
        axTask = nil
        restartTask?.cancel()
        restartTask = nil
        followTask?.cancel()
        followTask = nil
        staticTask?.cancel()
        staticTask = nil
        lastObservedMs = nil
        await streamer?.stop()
        streamer = nil
        ring = FrameRing(seconds: 20)
    }
}
