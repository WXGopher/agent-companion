// SPDX-License-Identifier: GPL-3.0-only
// Original vector artwork, rendered at every macOS icon size during packaging.
import AppKit

let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

func drawIcon() {
    let tile = NSBezierPath(roundedRect: NSRect(x: 64, y: 64, width: 896, height: 896), xRadius: 204, yRadius: 204)
    NSGradient(starting: NSColor(calibratedRed: 0.16, green: 0.21, blue: 0.25, alpha: 1),
               ending: NSColor(calibratedRed: 0.06, green: 0.09, blue: 0.12, alpha: 1))!.draw(in: tile, angle: -90)
    NSColor.white.withAlphaComponent(0.12).setStroke()
    tile.lineWidth = 10
    tile.stroke()

    // The thin top strip echoes the menu bar; the prompt identifies the CLI.
    NSColor(calibratedRed: 0.45, green: 0.90, blue: 0.74, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 224, y: 710, width: 576, height: 64), xRadius: 32, yRadius: 32).fill()
    NSColor(calibratedRed: 0.10, green: 0.14, blue: 0.18, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 424, y: 737, width: 176, height: 68), xRadius: 24, yRadius: 24).fill()

    NSColor(calibratedRed: 0.45, green: 0.90, blue: 0.74, alpha: 1).setStroke()
    let prompt = NSBezierPath()
    prompt.lineWidth = 58
    prompt.lineCapStyle = .round
    prompt.lineJoinStyle = .round
    prompt.move(to: NSPoint(x: 296, y: 570))
    prompt.line(to: NSPoint(x: 444, y: 446))
    prompt.line(to: NSPoint(x: 296, y: 322))
    prompt.stroke()
    NSColor(calibratedWhite: 0.94, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 542, y: 293, width: 192, height: 58), xRadius: 29, yRadius: 29).fill()
}

for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        let transform = AffineTransform(scale: CGFloat(pixels) / 1024)
        (transform as NSAffineTransform).concat()
        drawIcon()
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let path = output.appendingPathComponent("icon_\(size)x\(size)\(suffix).png")
        try bitmap.representation(using: .png, properties: [:])!.write(to: path)
    }
}
