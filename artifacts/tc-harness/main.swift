// 转码管线独立复现台(field r27 排障):真 AVAssetReader+VT+AudioConverter
// 泵跑一个真视频,段落盘 + 逐段字节账打印。库代码零改动。
import Foundation


let args = CommandLine.arguments
guard args.count >= 3 else {
    print("usage: harness <video> <outDir>")
    exit(2)
}
let src = args[1]
let outDir = args[2]
try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

var buf = [CChar](repeating: 0, count: Int(MAXPATHLEN))
guard realpath(src, &buf) != nil else { print("realpath fail"); exit(1) }
let path = String(cString: buf)
let fd = open(path, O_RDONLY)
guard fd >= 0 else { print("open fail \(errno)"); exit(1) }

let t = IOSRenditionTranscoder()
let name = (path as NSString).lastPathComponent
var segCount = 0
var byteLog: [(Int, Int)] = []
do {
    try await t.transcode(
        fd: fd, displayName: name, startAtMs: 0, sessionDir: outDir,
        notifier: { index, segPath in
            let size = (try? FileManager.default.attributesOfItem(atPath: segPath)[.size] as? Int) ?? 0
            byteLog.append((index, size ?? 0))
            segCount += 1
        },
        isCancelled: { false })
    print("TRANSCODE OK: \(segCount) segments")
} catch {
    print("TRANSCODE FAILED: \(error)")
}
for (i, sz) in byteLog.sorted(by: { $0.0 < $1.0 }) {
    print("  seg[\(i)] = \(sz) bytes")
}
// 逐段 trun 样表账
for (i, _) in byteLog.sorted(by: { $0.0 < $1.0 }) {
    let p = "\(outDir)/\(i).m4s"
    guard let data = FileManager.default.contents(atPath: p) else { continue }
    guard let report = MuxProbe.probe([UInt8](data)) else { print("  seg[\(i)] probe fail"); continue }
    print("  seg[\(i)]: vSamples=\(report.vCount) vSizeTotal=\(report.vSizeTotal) aSamples=\(report.aCount) aSizeTotal=\(report.aSizeTotal)")
}

exit(0)

// —— 段盒面探针(trun 样表账;简单走盒) ——
enum MuxProbe {
    static func u32(_ d: [UInt8], _ at: Int) -> Int {
        Int(d[at]) << 24 | Int(d[at+1]) << 16 | Int(d[at+2]) << 8 | Int(d[at+3])
    }
    static func probe(_ d: [UInt8]) -> (vCount: Int, vSizeTotal: Int, aCount: Int, aSizeTotal: Int)? {
        var truns: [[Int]] = []
        func walk(_ start: Int, _ end: Int, inTraf: Bool) {
            var pos = start
            while pos + 8 <= end {
                let size = u32(d, pos)
                guard size >= 8, pos + size <= end else { return }
                let typ = String(bytes: d[pos+4..<pos+8], encoding: .ascii) ?? ""
                if typ == "moof" || typ == "traf" {
                    walk(pos + 8, pos + size, inTraf: typ == "traf" || inTraf)
                } else if typ == "trun" && inTraf {
                    let count = u32(d, pos + 12)
                    var sizes: [Int] = []
                    var q = pos + 24
                    for _ in 0..<count {
                        sizes.append(u32(d, q + 4))
                        q += 12
                    }
                    truns.append(sizes)
                }
                pos += size
            }
        }
        walk(0, d.count, inTraf: false)
        guard truns.count >= 1 else { return nil }
        let v = truns[0]
        let a = truns.count > 1 ? truns[1] : []
        return (v.count, v.reduce(0, +), a.count, a.reduce(0, +))
    }
}
