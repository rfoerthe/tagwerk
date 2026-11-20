// Tagwerk – zeichnet den Hintergrund des DMG-Fensters: eine helle Fläche mit dem
// App-Namen, einem Pfeil vom App-Symbol zum Ordner „Programme“ und einem Hinweis.
// Die beiden Symbole selbst setzt der Finder obendrauf (siehe build.sh).
// Aufruf: swift tools/MakeDMGBackground.swift <Ausgabe.png>
import AppKit

// Das Fenster ist 600 × 400 pt groß; gerendert wird in 2× für Retina.
let scale: CGFloat = 2
let widthPt: CGFloat = 600, heightPt: CGFloat = 400
let px = Int(widthPt * scale), py = Int(heightPt * scale)

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "dmg-background.png"

guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: py,
                                 bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                                 isPlanar: false, colorSpaceName: .deviceRGB,
                                 bytesPerRow: 0, bitsPerPixel: 0),
      let ctx = NSGraphicsContext(bitmapImageRep: rep) else { exit(1) }
rep.size = NSSize(width: widthPt, height: heightPt)

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = ctx
ctx.cgContext.scaleBy(x: scale, y: scale)   // in Punkten zeichnen, Ausgabe in 2×

let full = NSRect(x: 0, y: 0, width: widthPt, height: heightPt)

// Sanfter, heller Verlauf von oben nach unten.
NSGradient(colors: [
    NSColor(calibratedWhite: 0.99, alpha: 1),
    NSColor(calibratedWhite: 0.94, alpha: 1),
])?.draw(in: full, angle: -90)

// Titel (Ursprung unten links – großes y liegt oben).
let title = "Tagwerk"
let titleAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 26, weight: .bold),
    .foregroundColor: NSColor(calibratedWhite: 0.12, alpha: 1),
]
let titleSize = title.size(withAttributes: titleAttrs)
title.draw(at: NSPoint(x: (widthPt - titleSize.width) / 2, y: heightPt - 70), withAttributes: titleAttrs)

// Hinweis unten.
let subtitle = "Tagwerk in den Ordner „Programme“ ziehen"
let subAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 12, weight: .regular),
    .foregroundColor: NSColor(calibratedWhite: 0.45, alpha: 1),
]
let subSize = subtitle.size(withAttributes: subAttrs)
subtitle.draw(at: NSPoint(x: (widthPt - subSize.width) / 2, y: 54), withAttributes: subAttrs)

// Pfeil zwischen den Symbolen (Mitten bei x ≈ 150 und x ≈ 450, y ≈ 200 von oben –
// im 400 pt hohen Bild also ebenfalls y ≈ 200 von unten).
let arrowY: CGFloat = heightPt - 200
let arrow = NSBezierPath()
arrow.lineWidth = 9
arrow.lineCapStyle = .round
arrow.lineJoinStyle = .round
arrow.move(to: NSPoint(x: 250, y: arrowY))
arrow.line(to: NSPoint(x: 348, y: arrowY))
arrow.move(to: NSPoint(x: 330, y: arrowY + 16))
arrow.line(to: NSPoint(x: 350, y: arrowY))
arrow.line(to: NSPoint(x: 330, y: arrowY - 16))
NSColor(calibratedRed: 0.40, green: 0.34, blue: 0.78, alpha: 0.9).setStroke()
arrow.stroke()

NSGraphicsContext.restoreGraphicsState()

guard let data = rep.representation(using: .png, properties: [:]) else { exit(1) }
do {
    try data.write(to: URL(fileURLWithPath: out))
} catch {
    FileHandle.standardError.write("DMG-Hintergrund konnte nicht geschrieben werden: \(error)\n".data(using: .utf8)!)
    exit(1)
}
