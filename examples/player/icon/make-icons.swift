// Draws the player's icon and writes it for every app:
//
//   swift icon/make-icons.swift      (from examples/player, on a Mac)
//
// - apple/SF2Player/Assets.xcassets/AppIcon.appiconset: iOS (full bleed) and
//   macOS (a rounded square with a margin, as macOS icons are drawn)
// - android/app/src/main/res: the adaptive icon's foreground
// - windows/assets/sf2player.ico

import AppKit

let top = NSColor(srgbRed: 0.35, green: 0.30, blue: 0.95, alpha: 1)
let bottom = NSColor(srgbRed: 0.62, green: 0.22, blue: 0.85, alpha: 1)

/// The glyph: a music note over three waveform bars.
func drawGlyph(in rect: CGRect) {
    NSColor.white.setFill()
    let s = rect.width
    func x(_ v: CGFloat) -> CGFloat { rect.minX + v * s }
    func y(_ v: CGFloat) -> CGFloat { rect.minY + v * s }
    // Note heads.
    NSBezierPath(ovalIn: CGRect(x: x(0.20), y: y(0.22), width: s * 0.22, height: s * 0.17)).fill()
    NSBezierPath(ovalIn: CGRect(x: x(0.56), y: y(0.30), width: s * 0.22, height: s * 0.17)).fill()
    // Stems and beam.
    NSBezierPath(rect: CGRect(x: x(0.36), y: y(0.30), width: s * 0.06, height: s * 0.48)).fill()
    NSBezierPath(rect: CGRect(x: x(0.72), y: y(0.38), width: s * 0.06, height: s * 0.48)).fill()
    let beam = NSBezierPath()
    beam.move(to: CGPoint(x: x(0.36), y: y(0.78)))
    beam.line(to: CGPoint(x: x(0.78), y: y(0.86)))
    beam.line(to: CGPoint(x: x(0.78), y: y(0.74)))
    beam.line(to: CGPoint(x: x(0.36), y: y(0.66)))
    beam.close()
    beam.fill()
}

func gradient(_ path: NSBezierPath) {
    NSGradient(starting: top, ending: bottom)!.draw(in: path, angle: -60)
}

enum Style { case fullBleed, mac, foreground }

func render(_ size: Int, _ style: Style) -> Data {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4,
        hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let s = CGFloat(size)
    let full = CGRect(x: 0, y: 0, width: s, height: s)
    switch style {
    case .fullBleed:
        gradient(NSBezierPath(rect: full))
        drawGlyph(in: full.insetBy(dx: s * 0.18, dy: s * 0.18))
    case .mac:
        // Apple's grid: an 824/1024 rounded square, centred.
        let body = full.insetBy(dx: s * 100 / 1024, dy: s * 100 / 1024)
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.3)
        shadow.shadowBlurRadius = s * 0.02
        shadow.shadowOffset = NSSize(width: 0, height: -s * 0.01)
        NSGraphicsContext.saveGraphicsState()
        shadow.set()
        gradient(NSBezierPath(roundedRect: body, xRadius: body.width * 0.225, yRadius: body.width * 0.225))
        NSGraphicsContext.restoreGraphicsState()
        drawGlyph(in: body.insetBy(dx: body.width * 0.18, dy: body.width * 0.18))
    case .foreground:
        // Android's adaptive icon: keep the glyph inside the 66% safe zone.
        drawGlyph(in: full.insetBy(dx: s * 0.27, dy: s * 0.27))
    }
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

func write(_ data: Data, _ path: String) {
    let url = URL(fileURLWithPath: path)
    try! FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try! data.write(to: url)
}

// Apple.
let appIcon = "apple/SF2Player/Assets.xcassets/AppIcon.appiconset"
var images: [[String: String]] = []
write(render(1024, .fullBleed), "\(appIcon)/ios-1024.png")
images.append(["idiom": "universal", "platform": "ios", "size": "1024x1024", "filename": "ios-1024.png"])
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = "mac-\(points)@\(scale)x.png"
        write(render(points * scale, .mac), "\(appIcon)/\(name)")
        images.append(["idiom": "mac", "size": "\(points)x\(points)", "scale": "\(scale)x", "filename": name])
    }
}
let contents: [String: Any] = ["images": images, "info": ["author": "xcode", "version": 1]]
write(try! JSONSerialization.data(withJSONObject: contents, options: [.prettyPrinted, .sortedKeys]),
      "\(appIcon)/Contents.json")

// Android: the foreground at each density (108 dp).
for (density, scale) in [("mdpi", 1.0), ("hdpi", 1.5), ("xhdpi", 2.0), ("xxhdpi", 3.0), ("xxxhdpi", 4.0)] {
    write(render(Int(108 * scale), .foreground), "android/app/src/main/res/mipmap-\(density)/ic_launcher_foreground.png")
}

// Windows: an .ico of PNGs.
let icoSizes = [16, 24, 32, 48, 64, 128, 256]
let pngs = icoSizes.map { render($0, .mac) }
var ico = Data([0, 0, 1, 0, UInt8(icoSizes.count), 0])
var offset = 6 + 16 * icoSizes.count
func le16(_ v: Int) -> [UInt8] { [UInt8(v & 0xff), UInt8(v >> 8 & 0xff)] }
func le32(_ v: Int) -> [UInt8] { le16(v & 0xffff) + le16(v >> 16) }
for (size, png) in zip(icoSizes, pngs) {
    let side = UInt8(size >= 256 ? 0 : size)
    ico.append(contentsOf: [side, side, 0, 0] + le16(1) + le16(32) + le32(png.count) + le32(offset))
    offset += png.count
}
for png in pngs { ico.append(png) }
write(ico, "windows/assets/sf2player.ico")
write(render(256, .mac), "windows/assets/icon-256.png")
print("icons written")
