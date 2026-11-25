/// fMP4/CMAF 分段 muxer 宿主单测(V4-1 T14;纯字节装配面——真解码/编码面
/// 归实机冒烟,AVAssetReader/VTCompressionSession 无 macOS 宿主档):init
/// 段盒子结构(ftyp 品牌/moov 双轨/avcC 记录/esds ASC)、内容段 moof/mdat
/// 装配(tfdt 基点/trun 样表/mdat 载荷)、分段参数(TranscodeParams 与
/// daemon RenditionConfig 同值锚 + 1080p 封顶缩放 + 切段下限)、段界规则与
/// ASC 种子(T13 评审 Critical #1 / Important #2 同款钉面)、Annex-B →
/// AVCC 转换与 avcC 记录装配(SPS/PPS 面——VT 参数集输入形)。
import Foundation
@testable import MTDSSKit
import XCTest

final class SegmentMuxerTests: XCTestCase {

    // ── 盒子遍历小件(测试专用;大端手解)─────────────────────────────────

    private func be32(_ d: [UInt8], _ off: Int) -> UInt32 {
        UInt32(d[off]) << 24 | UInt32(d[off + 1]) << 16 | UInt32(d[off + 2]) << 8 | UInt32(d[off + 3])
    }

    private func be64(_ d: [UInt8], _ off: Int) -> UInt64 {
        UInt64(be32(d, off)) << 32 | UInt64(be32(d, off + 4))
    }

    private func i32(_ d: [UInt8], _ off: Int) -> Int32 { Int32(bitPattern: be32(d, off)) }

    private func fourcc(_ d: [UInt8], _ off: Int) -> String {
        String(bytes: d[off..<(off + 4)], encoding: .ascii)!
    }

    private func boxesIn(_ data: [UInt8]) -> [(type: String, body: [UInt8])] {
        var out: [(type: String, body: [UInt8])] = []
        var i = 0
        while i + 8 <= data.count {
            let size = Int(be32(data, i))
            XCTAssertTrue(size >= 8, "盒子 size 须 ≥ 8(测试面受控字节)")
            out.append((fourcc(data, i + 4), Array(data[(i + 8)..<(i + size)])))
            i += size
        }
        XCTAssertEqual(i, data.count, "盒子遍历须恰尽")
        return out
    }

    private func box(_ type: String, in data: [UInt8]) -> (type: String, body: [UInt8]) {
        boxesIn(data).first { $0.type == type }!
    }

    // ── 装配面 ────────────────────────────────────────────────────────────

    /// SPS(4 载荷)+ PPS(5 载荷)——VT `CMVideoFormatDescriptionGetH264
    /// ParameterSetAtIndex` 面(iOS 输入形;Android csd-0 Annex-B 面的对应物)。
    private static let sps: [UInt8] = [0x67, 0x64, 0x00, 0x28]
    private static let pps: [UInt8] = [0x68, 0xEB, 0xEC, 0xB2, 0x2C]

    private func muxer() -> SegmentMuxer {
        SegmentMuxer(
            video: VideoTrackSpec(
                width: 1920, height: 1080,
                avcC: try! avcCRecord(sps: Self.sps, ppss: [Self.pps]),
                fps: 30),
            audio: AudioTrackSpec(sampleRate: 48_000, channels: 2, asc: [0x12, 0x10]))
    }

    /// 假编码样(Annex-B:起始码分隔两 NAL——管线面转 AVCC 后入 mux)。
    private func annexBSample() -> [UInt8] {
        [0, 0, 0, 1, 0x41, 0x9A, 0x2B, 0, 0, 0, 1, 0x41, 0x17, 0x08]
    }

    // ── init 段 ───────────────────────────────────────────────────────────

    func testInitSegmentBoxStructure() throws {
        let initSeg = muxer().initSegment()
        let top = boxesIn(initSeg)
        XCTAssertEqual(top.map(\.type), ["ftyp", "moov"])

        // ftyp:iso6 + 兼容面(isom/iso6/avc1/mp41)
        let ftyp = top[0]
        XCTAssertEqual(ftyp.body.count, 24) // major(4)+minor(4)+4×brand
        XCTAssertEqual(fourcc(ftyp.body, 0), "iso6")
        XCTAssertEqual(be32(ftyp.body, 4), 0)
        XCTAssertEqual(fourcc(ftyp.body, 8), "isom")
        XCTAssertEqual(fourcc(ftyp.body, 12), "iso6")
        XCTAssertEqual(fourcc(ftyp.body, 16), "avc1")
        XCTAssertEqual(fourcc(ftyp.body, 20), "mp41")

        // moov:mvhd + 2 trak + mvex(fMP4 必需——field 修)
        let moov = top[1]
        let kids = boxesIn(moov.body)
        XCTAssertEqual(kids.map(\.type), ["mvhd", "trak", "trak", "mvex"])
        XCTAssertEqual(kids[0].body.count, 100) // mvhd 载荷(v0 定长)

        // mvex:在场轨道各一枚 trex(默认值 0 + 缺省样描述索引 1;无 trex
        // = Chromium MSE/ffmpeg 拒段——field 修钉面)
        let mvex = kids[3]
        let trexes = boxesIn(mvex.body)
        XCTAssertEqual(trexes.map(\.type), ["trex", "trex"], "双轨双 trex")
        for (i, trex) in trexes.enumerated() {
            XCTAssertEqual(trex.body.count, 24)
            XCTAssertEqual(be32(trex.body, 0), UInt32(i + 1), "track_ID = \(i + 1)")
            XCTAssertEqual(be32(trex.body, 4), 1, "default_sample_description_index")
            XCTAssertEqual(be32(trex.body, 8), 0, "default_sample_duration")
            XCTAssertEqual(be32(trex.body, 12), 0, "default_sample_size")
            XCTAssertEqual(be32(trex.body, 16), 0, "default_sample_flags")
        }

        // 视频 trak:tkhd track_ID=1;width/height 16.16 = 1920/1080
        let vTkhd = box("tkhd", in: kids[1].body)
        XCTAssertEqual(vTkhd.body.count, 84)
        XCTAssertEqual(be32(vTkhd.body, 12), 1) // track_ID
        XCTAssertEqual(be32(vTkhd.body, 76), UInt32(1920) << 16) // width 16.16
        XCTAssertEqual(be32(vTkhd.body, 80), UInt32(1080) << 16) // height 16.16

        // 视频轨 avcC 记录(SPS/PPS → 长度前缀记录;avcCRecord 纯函数产物)
        let avcC = avccOf(kids[1])
        let expectedAvcC: [UInt8] = [
            1, // configurationVersion
            0x64, 0x00, 0x28, // profile/compatible/level(取自 SPS[1..3])
            0xFF, // lengthSizeMinusOne = 3
            0xE1, // numSPS = 1(field 修:0xE0 的 numSPS 位=0「零参数集」
            // avcC,解析器拒建流)
            0, 4, 0x67, 0x64, 0x00, 0x28, // SPS
            1, // numPPS
            0, 5, 0x68, 0xEB, 0xEC, 0xB2, 0x2C, // PPS
        ]
        XCTAssertEqual(avcC, expectedAvcC)

        // 音频 trak:esds 含 ASC(直装)+ objectTypeIndication = AAC
        let esds = esdsOf(kids[2])
        let asc: [UInt8] = [0x12, 0x10]
        XCTAssertTrue(esds.contains(subarray: asc), "esds 载荷含 ASC")
        XCTAssertEqual(esds[11], 0x40, "esds objectTypeIndication = AAC(0x40)")
    }

    /// trak → mdia → minf → stbl → stsd 深走,取 avcC 记录(stsd 条目自
    /// version/flags+entry_count 之后起盒;avc1 采样条目 78 字节固定头后
    /// 才是子盒面)。
    private func avccOf(_ trak: (type: String, body: [UInt8])) -> [UInt8] {
        let mdia = box("mdia", in: trak.body)
        let minf = box("minf", in: mdia.body)
        let stbl = box("stbl", in: minf.body)
        let stsd = box("stsd", in: stbl.body)
        let entry = boxesIn(Array(stsd.body[8...])).first!
        XCTAssertEqual(entry.type, "avc1")
        return box("avcC", in: Array(entry.body[78...])).body
    }

    /// trak → … → stsd,取 mp4a 的 esds 载荷(mp4a 采样条目 28 字节固定头
    /// 后才是 esds 子盒)。
    private func esdsOf(_ trak: (type: String, body: [UInt8])) -> [UInt8] {
        let mdia = box("mdia", in: trak.body)
        let minf = box("minf", in: mdia.body)
        let stbl = box("stbl", in: minf.body)
        let stsd = box("stsd", in: stbl.body)
        let entry = boxesIn(Array(stsd.body[8...])).first!
        XCTAssertEqual(entry.type, "mp4a")
        return box("esds", in: Array(entry.body[28...])).body
    }

    // ── 内容段 ────────────────────────────────────────────────────────────

    func testContentSegmentAssembly() throws {
        let m = muxer()
        m.beginFragment(seq: 3)
        // mux 契约 = 样恒 AVCC 形入(Annex-B 形设备的转换在管线边界单点)
        m.addVideoSample(ptsUs: 900_000, keyFrame: true,
                         data: AnnexB.avccSample(from: annexBSample()))
        m.addVideoSample(ptsUs: 930_000, keyFrame: false,
                         data: AnnexB.avccSample(from: annexBSample()))
        m.addAudioSample(ptsUs: 900_333, data: [1, 2, 3])
        m.addAudioSample(ptsUs: 902_390, data: [4, 5, 6])

        let seg = try m.finishFragment()
        let top = boxesIn(seg)
        XCTAssertEqual(top.map(\.type), ["moof", "mdat"])

        // moof → mfhd(seq=3)+ traf(v)+ traf(a)
        let kids = boxesIn(top[0].body)
        XCTAssertEqual(kids.map(\.type), ["mfhd", "traf", "traf"])
        XCTAssertEqual(be32(kids[0].body, 4), 3) // sequence_number

        // 视频 traf:tfhd track 1 + tfdt v1(基点 = 首样 pts 900000us → 81000)
        let vTraf = kids[1]
        XCTAssertEqual(i32(box("tfhd", in: vTraf.body).body, 0), 0x020000) // default-base-is-moof
        XCTAssertEqual(be32(box("tfhd", in: vTraf.body).body, 4), 1) // track_ID
        let vTfdt = box("tfdt", in: vTraf.body)
        XCTAssertEqual(i32(vTfdt.body, 0) >> 24, 1) // version 1
        XCTAssertEqual(be64(vTfdt.body, 4), 81_000)
        let vTrun = box("trun", in: vTraf.body)
        XCTAssertEqual(i32(vTrun.body, 0), 0x000701) // data-offset+dur+size+flags
        XCTAssertEqual(be32(vTrun.body, 4), 2) // 样数

        // 音频 traf:tfhd track 2 + tfdt 基点 = 900333us → 43216(48000Hz)
        let aTraf = kids[2]
        XCTAssertEqual(be32(box("tfhd", in: aTraf.body).body, 4), 2)
        XCTAssertEqual(be64(box("tfdt", in: aTraf.body).body, 4), 43_216)

        // mdat:视频样 AVCC 长度前缀(Annex-B 已转)+ 音频样直拷
        let mdat = top[1]
        let v1 = AnnexB.avccSample(from: annexBSample()) // 2×(4 长度+NAL)
        XCTAssertEqual(
            Int64(mdat.body.count),
            Int64(v1.count * 2 + 3 + 3),
            "mdat = 2 视频样(AVCC)+ 2 音频样")
        XCTAssertEqual(
            be32(mdat.body, 0), 3,
            "AVCC 首长度前缀 = 首 NAL 3 载荷")
    }

    // ── 分段参数(daemon RenditionConfig 同值锚)──────────────────────────

    func testSegmentationParamsAnchor() {
        XCTAssertEqual(TranscodeParams.segmentMs, 4_000)
        XCTAssertEqual(TranscodeParams.targetHeight, 1080)
        XCTAssertEqual(TranscodeParams.bitrateMinBps, 4_000_000)
        XCTAssertEqual(TranscodeParams.bitrateMaxBps, 6_000_000)
        XCTAssertTrue(
            (TranscodeParams.bitrateMinBps...TranscodeParams.bitrateMaxBps)
                .contains(TranscodeParams.targetBitrateBps),
            "实编码率须在窗内")
        XCTAssertTrue(
            TranscodeParams.segmentCutMinMs < TranscodeParams.segmentMs,
            "切段下限 < 段长(段长稳定窗)")
    }

    func testTargetDims1080CapEvenAlign() {
        XCTAssertEqual(targetDims(srcW: 3840, srcH: 2160).width, 1920)
        XCTAssertEqual(targetDims(srcW: 3840, srcH: 2160).height, 1080)
        XCTAssertEqual(targetDims(srcW: 1920, srcH: 1080).width, 1920)
        XCTAssertEqual(targetDims(srcW: 1920, srcH: 1080).height, 1080)
        XCTAssertEqual(targetDims(srcW: 854, srcH: 480).width, 854)
        XCTAssertEqual(targetDims(srcW: 854, srcH: 480).height, 480)
        XCTAssertEqual(targetDims(srcW: 1, srcH: 1).width, 2)
        XCTAssertEqual(targetDims(srcW: 1, srcH: 1).height, 2)
    }

    // ── 段界规则与 ASC 种子(T13 评审 Critical #1 / Important #2 同款钉面)─

    func testCutBeforeSyncSegmentIndependence() throws {
        // 纯函数钉面(cutBeforeSync):空段首样恒入(段 1 首 IDR 开段);
        // 非空段见同步且累计 ≥ 下限 = 切;非同步样/累计不足恒不切
        XCTAssertTrue(cutBeforeSync(hasSamples: true, accumulatedMs: 4_000, isSync: true, minMs: 2_000))
        XCTAssertFalse(cutBeforeSync(hasSamples: false, accumulatedMs: 0, isSync: true, minMs: 2_000))
        XCTAssertFalse(cutBeforeSync(hasSamples: true, accumulatedMs: 4_000, isSync: false, minMs: 2_000))
        XCTAssertFalse(cutBeforeSync(hasSamples: true, accumulatedMs: 1_000, isSync: true, minMs: 2_000))

        // SegmentMuxer 级重放管线用法:段 1 = P、P(尾样非同步)→ 同步样判
        // 切先收段 → 段 2 首样 = sync。trun 样表逐样 flags 断言(段独立面)。
        let m = muxer()
        m.beginFragment(seq: 1)
        m.addVideoSample(ptsUs: 0, keyFrame: false,
                         data: AnnexB.avccSample(from: annexBSample()))
        m.addVideoSample(ptsUs: 1_000_000, keyFrame: false,
                         data: AnnexB.avccSample(from: annexBSample()))
        // 累计 = 2000ms(pts 跨 1s + 末样时长外推 1s)= 恰达下限
        XCTAssertEqual(m.videoAccumulatedMs(), 2_000)
        XCTAssertTrue(
            cutBeforeSync(
                hasSamples: m.fragmentHasSamples, accumulatedMs: m.videoAccumulatedMs(),
                isSync: true, minMs: TranscodeParams.segmentCutMinMs),
            "第三样(同步)到达应触发切段判据")
        let seg1 = try m.finishFragment()
        m.beginFragment(seq: 2)
        m.addVideoSample(ptsUs: 2_000_000, keyFrame: true, data: annexBSample())
        let seg2 = try m.finishFragment()
        XCTAssertEqual(
            videoTrunFlags(seg1), [0x0101_0000, 0x0101_0000],
            "段 1 = 两 P 帧(无尾同步样)")
        XCTAssertEqual(
            videoTrunFlags(seg2), [0x0200_0000],
            "段 2 首样 = sync(关键帧开段)")
    }

    /// moof → 首 traf(video)→ trun 样表 flags(逐样;trun 载荷 =
    /// version/flags 4 + count 4 + data_offset 4 + 逐样 dur/size/flags 12)。
    private func videoTrunFlags(_ seg: [UInt8]) -> [UInt32] {
        let moof = box("moof", in: seg)
        let traf = boxesIn(moof.body).first { $0.type == "traf" }!
        let trun = box("trun", in: traf.body)
        let count = Int(be32(trun.body, 4))
        return (0..<count).map { be32(trun.body, 12 + $0 * 12 + 8) }
    }

    func testAudioAscSeed() {
        // 转编臂源 csd 非 ASC 形(如 AC3 的 dac3 盒)——误种会错填 esds 且
        // 令 muxer「音频 csd 齐」等待条件假满足(评审 Important #2)
        let asc: [UInt8] = [0x12, 0x10]
        XCTAssertEqual(audioAscSeedFor(passthrough: true, sourceCsd: asc), asc)
        XCTAssertNil(audioAscSeedFor(passthrough: false, sourceCsd: asc))
        XCTAssertNil(audioAscSeedFor(passthrough: false, sourceCsd: nil))
    }

    // ── Annex-B → AVCC 转换与 avcC 记录装配 ──────────────────────────────

    func testAnnexBSplitAndAvccRecord() throws {
        // 拆 NAL(3/4 字节起始码皆认)+ 样面长度前缀化
        let sample: [UInt8] = [0, 0, 1, 0x41, 0x02, 0, 0, 1, 0x41, 0x03]
        XCTAssertEqual(
            AnnexB.avccSample(from: sample),
            [0, 0, 0, 2, 0x41, 0x02, 0, 0, 0, 2, 0x41, 0x03])
        // avcC 记录(SPS/PPS 输入形——VT 参数集面)
        let record = try avcCRecord(
            sps: [0x67, 0x64, 0x00, 0x28],
            ppss: [[0x68, 0x11, 0x22]])
        XCTAssertEqual(
            record,
            [1, 0x64, 0x00, 0x28, 0xFF, 0xE1,
             0, 4, 0x67, 0x64, 0x00, 0x28,
             1, 0, 3, 0x68, 0x11, 0x22])
        // 空 SPS = 契约违规(记录必含参数集)
        XCTAssertThrowsError(try avcCRecord(sps: [], ppss: []))
    }

    // ── field 修两纯函数:样形态嗅探 + ASC 净化 ──────────────────────────

    func testSampleLooksAnnexB() {
        // Annex-B:3/4 字节起始码开头
        XCTAssertTrue(sampleLooksAnnexB([0, 0, 1, 0x06, 0x05]))
        XCTAssertTrue(sampleLooksAnnexB([0, 0, 0, 1, 0x67, 0x64]))
        // AVCC:首 4 字节长度前缀(macOS 26/iOS 26 VT 无 avcC 键设备面实
        // 测形——raw=12870 样钉面:00 00 00 3b 06 05 ...)
        XCTAssertFalse(sampleLooksAnnexB([0, 0, 0, 0x3B, 0x06, 0x05, 0x32]))
        XCTAssertFalse(sampleLooksAnnexB([0x00, 0x00, 0xDE, 0x02, 0x41]))
        // 短样:免转换透传(非 Annex-B)
        XCTAssertFalse(sampleLooksAnnexB([0, 0]))
        XCTAssertFalse(sampleLooksAnnexB([]))
        // 4 字节恰好 00 00 00 01 = Annex-B(3 字节判命中)
        XCTAssertTrue(sampleLooksAnnexB([0, 0, 1]))
    }

    func testSanitizeAudioAsc() {
        // 裸 ASC:原样透传(AAC-LC 44.1k 立体声经典 12 10)
        XCTAssertEqual(sanitizeAudioAsc([0x12, 0x10]), [0x12, 0x10])
        // 5 字节显式配置 ASC(实测树内 DSI:12 08 56 e5 00)
        XCTAssertEqual(sanitizeAudioAsc([0x12, 0x08, 0x56, 0xE5, 0x00]), [0x12, 0x08, 0x56, 0xE5, 0x00])
        // macOS 26 实测 cookie = 整棵 esds 树(扩展长度形;容器头 3/13B):
        // 03{ES_ID+flags 04{13B 体 05{ASC}} 06{02}} → 取 05 载荷
        let tree: [UInt8] = [
            0x03, 0x80, 0x80, 0x80, 0x25,
            0x00, 0x02, 0x00, // ES_ID + flags
            0x04, 0x80, 0x80, 0x80, 0x17,
            0x40, 0x15, // OTI + streamType
            0x00, 0x00, 0x00, // bufferSizeDB
            0x00, 0x01, 0x77, 0x00, // maxBitrate
            0x00, 0x01, 0x76, 0x36, // avgBitrate
            0x05, 0x80, 0x80, 0x80, 0x05,
            0x12, 0x08, 0x56, 0xE5, 0x00, // ← 真 ASC
            0x06, 0x80, 0x80, 0x80, 0x01,
            0x02,
        ]
        XCTAssertEqual(sanitizeAudioAsc(tree), [0x12, 0x08, 0x56, 0xE5, 0x00])
        // 短形长度树(经典 ffmpeg 形)同样取 DSI
        let shortTree: [UInt8] = [
            0x03, 0x19, 0x00, 0x02, 0x00,
            0x04, 0x11, 0x40, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x05, 0x02, 0x12, 0x10,
            0x06, 0x01, 0x02,
        ]
        XCTAssertEqual(sanitizeAudioAsc(shortTree), [0x12, 0x10])
        // 垃圾(AOT 0 形)= nil
        XCTAssertNil(sanitizeAudioAsc([0x03, 0x80, 0x80, 0x80]))
        XCTAssertNil(sanitizeAudioAsc([]))
    }
}

private extension Array where Element == UInt8 {
    func contains(subarray other: [UInt8]) -> Bool {
        guard !other.isEmpty, count >= other.count else { return false }
        return (0...(count - other.count)).contains { i in
            (0..<other.count).allSatisfy { j in self[i + j] == other[j] }
        }
    }
}
