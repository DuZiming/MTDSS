// MTDSS placeholder app icon generator (v2p5-t10b).
// 1024x1024 full-bleed opaque PNG, sRGB: blue vertical gradient + white
// down-arrow-into-tray glyph (~56% canvas). Usage: swift gen_icon.swift <out.png>
import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

let out = CommandLine.arguments[1]
let size = 1024
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
// Opaque RGB context: plain `.none` is not a supported CGBitmapContext format
// (returns nil); noneSkipLast is the standard opaque RGB8 layout and ImageIO
// encodes it as an alpha-free 24-bit PNG. iOS app icons must be opaque and
// full-bleed square (system masks them).
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
                    bytesPerRow: 0, space: cs,
                    bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!

// Background: #0A84FF (top) -> #0060DF (bottom). CG origin is bottom-left.
let top = CGColor(srgbRed: 0x0A/255.0, green: 0x84/255.0, blue: 0xFF/255.0, alpha: 1)
let bottom = CGColor(srgbRed: 0x00/255.0, green: 0x60/255.0, blue: 0xDF/255.0, alpha: 1)
let grad = CGGradient(colorsSpace: cs, colors: [top, bottom] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(grad, start: CGPoint(x: 0, y: CGFloat(size)),
                       end: CGPoint(x: 0, y: 0), options: [])

let white = CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1)
ctx.setFillColor(white)
ctx.setStrokeColor(white)

// Glyph geometry below is in CG coordinates (y up); design intent (y down):
//   shaft y 232..470, head base y 470 tip y 600, tray opening y 644 bottom 772.
// Arrow shaft: overlaps 12px into the head so the rounded bottom corners
// are buried (visible top corners keep a slight softness).
let shaft = CGPath(roundedRect: CGRect(x: 476, y: 542, width: 72, height: 250),
                   cornerWidth: 12, cornerHeight: 12, transform: nil)
ctx.addPath(shaft)
ctx.fillPath()

// Arrow head: triangle, base (404,554)-(620,554), tip (512,424).
let head = CGMutablePath()
head.move(to: CGPoint(x: 404, y: 554))
head.addLine(to: CGPoint(x: 620, y: 554))
head.addLine(to: CGPoint(x: 512, y: 424))
head.closeSubpath()
ctx.addPath(head)
ctx.fillPath()

// Tray: U-shaped stroke, opening up. Centerline x 242/782, top y 380,
// bottom y 252, corner radius 48; 58px stroke -> outer span x 213..811.
let tray = CGMutablePath()
tray.move(to: CGPoint(x: 242, y: 380))
tray.addLine(to: CGPoint(x: 242, y: 300))
tray.addArc(center: CGPoint(x: 290, y: 300), radius: 48,
            startAngle: .pi, endAngle: 1.5 * .pi, clockwise: false)
tray.addLine(to: CGPoint(x: 734, y: 252))
tray.addArc(center: CGPoint(x: 734, y: 300), radius: 48,
            startAngle: 1.5 * .pi, endAngle: 2 * .pi, clockwise: false)
tray.addLine(to: CGPoint(x: 782, y: 380))
ctx.addPath(tray)
ctx.setLineWidth(58)
ctx.setLineCap(.butt)
ctx.setLineJoin(.miter)
ctx.strokePath()

let img = ctx.makeImage()!
let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: out) as CFURL,
                                           UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, img, nil)
guard CGImageDestinationFinalize(dest) else {
  fatalError("PNG write failed: \(out)")
}
print("wrote \(out)")
