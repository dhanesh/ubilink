// OCR helper for scripts/probe.py's `ocr` step (GPUI Phase 0 R8; spec D6/TN5).
//
// Usage: ocr <image.png>
// Prints {"lines": [{"text": ..., "confidence": ..., "box": [x, y, w, h]}, ...]} on
// stdout, boxes in image pixels with a top-left origin (screencapture's PNG space), one
// entry per recognised line in reading order. Exit 2 on bad input or a Vision failure.
//
// Vision rather than a third-party OCR engine: it ships with macOS, so the probe stays
// stdlib Python plus the system Swift toolchain. probe.py compiles this once with
// `swiftc -O` into ~/Library/Caches/ubilink-probe/bin/ (keyed by the source hash).
// OCR is only evidence for colour/visual checks; text proofs read the in-process
// AccessKit tree (TN5), which needs no Screen Recording grant.

import Foundation
import ImageIO
import Vision

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(2)
}

let args = CommandLine.arguments
guard args.count == 2 else { fail("usage: ocr <image.png>") }
let url = URL(fileURLWithPath: args[1])
guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(src, 0, nil)
else { fail("cannot read image: \(args[1])") }

let request = VNRecognizeTextRequest()
// Accurate, not fast: probes assert exact strings, and a panel screenshot is small.
request.recognitionLevel = .accurate
// UI text is names, URIs and labels, not prose: language correction would "fix" them.
request.usesLanguageCorrection = false

let handler = VNImageRequestHandler(cgImage: image, options: [:])
do {
    try handler.perform([request])
} catch {
    fail("vision failed: \(error)")
}

let w = Double(image.width), h = Double(image.height)
var lines: [[String: Any]] = []
for obs in request.results ?? [] {
    guard let top = obs.topCandidates(1).first else { continue }
    // Vision boxes are normalised with a bottom-left origin; flip to image pixels.
    let b = obs.boundingBox
    let box = [b.minX * w, (1 - b.maxY) * h, b.width * w, b.height * h].map { ($0 * 10).rounded() / 10 }
    lines.append(["text": top.string, "confidence": Double(top.confidence), "box": box])
}
let out = try! JSONSerialization.data(withJSONObject: ["lines": lines], options: [])
FileHandle.standardOutput.write(out)
FileHandle.standardOutput.write("\n".data(using: .utf8)!)
