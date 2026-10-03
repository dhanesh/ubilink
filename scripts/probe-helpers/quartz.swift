// Quartz/AppKit helper for scripts/probe.py (GPUI Phase 0 R8): the desktop facts a
// stdlib-only Python cannot reach, one subcommand each, JSON on stdout, exit 2 on bad
// input. probe.py compiles this once with `swiftc -O` into
// ~/Library/Caches/ubilink-probe/bin/ (keyed by the source hash).
//
// None of these need the Accessibility grant: window bounds and owner pids come from
// CGWindowListCopyWindowInfo (names would need Screen Recording; we never ask for
// them), cursor warping is CGWarpMouseCursorPosition (no synthetic event is posted),
// and `screen-access` only *preflights* Screen Recording, it never prompts (TN5: a TCC
// prompt would stall an unattended verifier).
//
//   windows <pid>        on-screen windows owned by <pid>: number, layer, bounds
//   frontmost            the active app: {"pid", "name"}
//   displays             active displays: id, main, bounds (global points, top-left)
//   cursor               cursor location in the same global space
//   warp <x> <y>         move the cursor to a global point
//   pbcount              NSPasteboard.general.changeCount
//   screen-access        {"granted": bool} from CGPreflightScreenCaptureAccess
//   display-asleep       {"id", "asleep": bool} CGDisplayIsAsleep on the main display
//                        (probe.py doctor, R9: an asleep main display is a FAIL)
//   pixel <png> <x> <y> <w> <h> <points-width>
//                        mean sRGB colour of a region given in window points; the
//                        image's backing scale is its pixel width / <points-width>

import AppKit
import CoreGraphics
import Foundation
import ImageIO

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(2)
}

func emit(_ obj: Any) {
    let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
}

func rect(_ r: CGRect) -> [String: Double] {
    ["x": Double(r.origin.x), "y": Double(r.origin.y), "w": Double(r.width), "h": Double(r.height)]
}

func num(_ s: String) -> Double {
    guard let v = Double(s) else { fail("not a number: \(s)") }
    return v
}

let args = Array(CommandLine.arguments.dropFirst())
guard let cmd = args.first else {
    fail("usage: quartz <windows|frontmost|displays|cursor|warp|pbcount|screen-access|display-asleep|pixel> ...")
}

switch cmd {
case "windows":
    guard args.count == 2, let pid = Int(args[1]) else { fail("usage: quartz windows <pid>") }
    // nil means the window server gave no answer (no session, display asleep): fail,
    // never report it as "no windows", or `winframe gone` would pass on nothing.
    guard let info = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
        fail("CGWindowListCopyWindowInfo returned nil")
    }
    var out: [[String: Any]] = []
    for w in info {
        guard (w[kCGWindowOwnerPID as String] as? Int) == pid else { continue }
        var bounds = CGRect.zero
        if let b = w[kCGWindowBounds as String] as? NSDictionary {
            _ = CGRectMakeWithDictionaryRepresentation(b as CFDictionary, &bounds)
        }
        out.append([
            "number": w[kCGWindowNumber as String] as? Int ?? 0,
            "layer": w[kCGWindowLayer as String] as? Int ?? 0,
            "alpha": w[kCGWindowAlpha as String] as? Double ?? 1,
            "onscreen": w[kCGWindowIsOnscreen as String] as? Bool ?? false,
            "bounds": rect(bounds),
        ])
    }
    emit(out)

case "frontmost":
    // The active app, for the probe's focus-theft check: pid and name only (from
    // NSWorkspace, no TCC grant needed).
    let a = NSWorkspace.shared.frontmostApplication
    emit(["pid": Int(a?.processIdentifier ?? 0), "name": a?.localizedName ?? ""])

case "displays":
    var count: UInt32 = 0
    CGGetActiveDisplayList(0, nil, &count)
    var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
    CGGetActiveDisplayList(count, &ids, &count)
    emit(ids.prefix(Int(count)).map { id -> [String: Any] in
        ["id": Int(id), "main": CGDisplayIsMain(id) != 0, "bounds": rect(CGDisplayBounds(id))]
    })

case "cursor":
    let p = CGEvent(source: nil)?.location ?? .zero
    emit(["x": Double(p.x), "y": Double(p.y)])

case "warp":
    guard args.count == 3 else { fail("usage: quartz warp <x> <y>") }
    let p = CGPoint(x: num(args[1]), y: num(args[2]))
    let err = CGWarpMouseCursorPosition(p)
    // Warping detaches the mouse from the cursor for a moment; re-associate so the
    // next real mouse move does not jump.
    CGAssociateMouseAndMouseCursorPosition(1)
    guard err == .success else { fail("CGWarpMouseCursorPosition failed: \(err.rawValue)") }
    let now = CGEvent(source: nil)?.location ?? .zero
    emit(["x": Double(now.x), "y": Double(now.y)])

case "pbcount":
    emit(["change_count": NSPasteboard.general.changeCount])

case "screen-access":
    emit(["granted": CGPreflightScreenCaptureAccess()])

case "display-asleep":
    // CGMainDisplayID/CGDisplayIsAsleep rather than the first id from `displays`:
    // CGGetActiveDisplayList can still list a sleeping display as "active", so
    // doctor needs this explicit awake/asleep bit, not just a non-empty list.
    let id = CGMainDisplayID()
    emit(["id": Int(id), "asleep": CGDisplayIsAsleep(id) != 0])

case "pixel":
    guard args.count == 7 else { fail("usage: quartz pixel <png> <x> <y> <w> <h> <points-width>") }
    let url = URL(fileURLWithPath: args[1])
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(src, 0, nil)
    else { fail("cannot read image: \(args[1])") }
    let ptsWidth = num(args[6])
    guard ptsWidth > 0 else { fail("points-width must be > 0") }
    let scale = Double(image.width) / ptsWidth
    let px = CGRect(x: num(args[2]) * scale, y: num(args[3]) * scale,
                    width: max(1, num(args[4]) * scale), height: max(1, num(args[5]) * scale)).integral
    guard let crop = image.cropping(to: px) else { fail("region \(px) is outside the \(image.width)x\(image.height) image") }
    // Draw into an sRGB context so the numbers are sRGB whatever profile the capture
    // carries (screencapture tags it with the display's), matching the hex a probe names.
    let w = crop.width, h = crop.height
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    guard let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                              space: CGColorSpace(name: CGColorSpace.sRGB)!,
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { fail("cannot make an sRGB context") }
    ctx.draw(crop, in: CGRect(x: 0, y: 0, width: w, height: h))
    var sum = [0.0, 0.0, 0.0]
    for i in stride(from: 0, to: buf.count, by: 4) {
        for c in 0..<3 { sum[c] += Double(buf[i + c]) }
    }
    let n = Double(w * h)
    let rgb = sum.map { Int(($0 / n).rounded()) }
    emit(["rgb": rgb, "hex": String(format: "#%02x%02x%02x", rgb[0], rgb[1], rgb[2]),
          "scale": scale, "pixels": w * h])

default:
    fail("unknown subcommand: \(cmd)")
}
