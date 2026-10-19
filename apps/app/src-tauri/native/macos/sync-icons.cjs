#!/usr/bin/env node
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons() {
  if (process.platform !== "darwin") {
    throw new Error("macOS Dock 和托盘图标需要在 macOS 上生成");
  }
  return generateNativeIcons("macos", {
    inputs: [__filename],
    generate({ generated, output, root }) {
      fs.copyFileSync(path.join(generated, "32x32.png"), path.join(output, "32x32.png"));
      execFileSync("swift", ["-module-cache-path", path.join(root, ".tmp/swift-module-cache"),
        "-", path.join(root, "apps/icon/icon.png"), output],
        { cwd: root, input: swiftSource, stdio: ["pipe", "inherit", "inherit"] });
    },
  });
}

// Keep CoreGraphics rendering here so icon generation has a single maintained entry point.
const swiftSource = String.raw`import Foundation
import CoreGraphics
import ImageIO

let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1])
let icons = URL(fileURLWithPath: CommandLine.arguments[2])
try FileManager.default.createDirectory(at: icons, withIntermediateDirectories: true)

guard let source = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
    fatalError("Cannot read apps/icon/icon.png")
}

// Normalize the source's existing transparent margins so the visible artwork
// matches the Dock size, rather than adding a second inset around those margins.
let boundsContext = CGContext(
    data: nil, width: image.width, height: image.height, bitsPerComponent: 8,
    bytesPerRow: image.width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
)!
boundsContext.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
let rgba = boundsContext.data!.assumingMemoryBound(to: UInt8.self)
var left = image.width, bottom = image.height, right = 0, top = 0
for y in 0..<image.height {
    for x in 0..<image.width where rgba[(y * image.width + x) * 4 + 3] >= 128 {
        left = min(left, x)
        bottom = min(bottom, y)
        right = max(right, x + 1)
        top = max(top, y + 1)
    }
}
guard right > left && top > bottom else { fatalError("Source icon has no visible artwork") }
let visible = CGRect(x: left, y: bottom, width: right - left, height: top - bottom)

func drawArtwork(in context: CGContext, canvas: CGFloat, artwork: CGFloat) {
    let scale = artwork / max(visible.width, visible.height)
    context.clear(CGRect(x: 0, y: 0, width: canvas, height: canvas))
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(
        x: canvas / 2 - visible.midX * scale,
        y: canvas / 2 - visible.midY * scale,
        width: CGFloat(image.width) * scale,
        height: CGFloat(image.height) * scale
    ))
}

// Keep the rounded-square artwork inside an 856 px square on a 1024 px canvas.
// The transparent inset prevents it from looking oversized in the Dock.
let representations = [
    (16, 1, "icp4"), (16, 2, "ic11"),
    (32, 1, "icp5"), (32, 2, "ic12"),
    (128, 1, "ic07"), (128, 2, "ic13"),
    (256, 1, "ic08"), (256, 2, "ic14"),
    (512, 1, "ic09"), (512, 2, "ic10"),
]

func chunkHeader(_ type: String, _ length: Int) -> Data {
    var size = UInt32(length).bigEndian
    var header = Data(type.utf8)
    withUnsafeBytes(of: &size) { header.append(contentsOf: $0) }
    return header
}

var chunks = Data()
var tableOfContents = Data()
for (points, scale, type) in representations {
    let pixels = points * scale
    guard let context = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: pixels * 4,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { fatalError("Cannot create \(pixels) px canvas") }
    let canvas = CGFloat(pixels)
    let artwork = (canvas * 856 / 1024).rounded()
    drawArtwork(in: context, canvas: canvas, artwork: artwork)

    let png = NSMutableData()
    guard let result = context.makeImage(),
          let destination = CGImageDestinationCreateWithData(png, "public.png" as CFString, 1, nil) else {
        fatalError("Cannot encode \(pixels) px icon")
    }
    CGImageDestinationAddImage(destination, result, nil)
    guard CGImageDestinationFinalize(destination) else { fatalError("Cannot write \(pixels) px icon") }
    let header = chunkHeader(type, png.length + 8)
    tableOfContents.append(header)
    chunks.append(header)
    chunks.append(png as Data)
}

// ICNS stores PNG representations in typed chunks with big-endian lengths.
var output = chunkHeader("icns", 8 + 8 + tableOfContents.count + chunks.count)
output.append(chunkHeader("TOC ", tableOfContents.count + 8))
output.append(tableOfContents)
output.append(chunks)
try output.write(to: icons.appendingPathComponent("icon.icns"), options: .atomic)
print("Generated native/macos/icons/icon.icns (856/1024 artwork, transparent padding)")

// Render the same ClipRoam artwork for the 18 pt menu bar slot at 2x resolution.
// A 1 pt transparent inset keeps it clear of the menu bar edges.
let trayContext = CGContext(
    data: nil, width: 36, height: 36, bitsPerComponent: 8, bytesPerRow: 36 * 4,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!
drawArtwork(in: trayContext, canvas: 36, artwork: 32)
let trayURL = icons.appendingPathComponent("tray-macos.png")
guard let trayImage = trayContext.makeImage(),
      let trayDestination = CGImageDestinationCreateWithURL(trayURL as CFURL, "public.png" as CFString, 1, nil) else {
    fatalError("Cannot create macOS tray icon")
}
CGImageDestinationAddImage(trayDestination, trayImage, nil)
guard CGImageDestinationFinalize(trayDestination) else { fatalError("Cannot write macOS tray icon") }
print("Generated native/macos/icons/tray-macos.png (same ClipRoam artwork, 18 pt at 2x)")
`;

module.exports = { syncIcons };
if (require.main === module) syncIcons();
