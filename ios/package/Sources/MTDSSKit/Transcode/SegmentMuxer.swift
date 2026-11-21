/// fMP4/CMAF 分段 muxer(V4-1 T14;iOS 面与 Android `transcode/SegmentMuxer.kt`
/// 同构,demon 侧 `preview::rendition::RenditionConfig` 契约为真源;纯字节
/// 装配,macOS 宿主 swift test 可测——真解码/编码面归实机冒烟,AVAssetReader
/// /VTCompressionSession 无宿主档):init 段(`ftyp+moov`,T12 段序约定
/// index 0)+ 内容段(`moof+mdat`,每段独立起头可解;视频 90000 timescale /
/// 音频 = 采样率 timescale,tfdt v1,trun v0 带 data-offset,
/// `default-base-is-moof`)。样面:视频轨 AVC(**恒 AVCC 长度前缀形**入 mux
/// ——VT 输出 Annex-B 形的设备面由管线经 `AnnexB.avccSample` 单点转换,
/// avcC 记录由 `avcCRecord(sps:pps:)` 自 VT 参数集装配;Android csd-0
/// Annex-B 面的对应输入形),音频轨 AAC(原始帧直拷)。
///
/// 用法:构造 → `initSegment()` 出 init 段 → `beginFragment`/
/// `addVideoSample`/`addAudioSample`/…/`finishFragment` 逐段循环(段边界 =
/// 关键帧,由管线经 `cutBeforeSync` 纯函数决定;本类只管装配)。**混段**:
/// 视频+音频样同段(单 moof 双 traf)——T12 playlist 单 rendition 形
/// (MAP + seg/{i}.m4s)直接可吃。

import Foundation

/// 转码形态参数(V4-1 T14;daemon 侧 `preview::rendition::RenditionConfig`
/// 同值锚——段长/高度/码率窗,改值须双侧同步,rendition.rs 常量断言测试
/// 钉面。码率窗 4–6 Mbps 内取中为实编值;段切下限防场景切同步帧早切)。
public enum TranscodeParams {
    /// 段长毫秒(daemon RenditionConfig.SEGMENT_MS 同值;playlist
    /// TARGETDURATION=4 的原料)。
    public static let segmentMs: Int64 = 4_000

    /// 目标高度封顶(1080p;低分辨率源不放大)。
    public static let targetHeight: Int = 1080

    /// 目标码率窗(daemon 同值锚;实编值取窗内常数)。
    public static let bitrateMinBps: Int = 4_000_000
    public static let bitrateMaxBps: Int = 6_000_000

    /// 实编码率(窗内取中;V1 常量,实机冒烟可调)。
    public static let targetBitrateBps: Int = 5_000_000

    /// 音频转码码率(AAC-LC;直通轨不适用)。
    public static let audioBitrateBps: Int = 128_000

    /// 切段下限毫秒:视频同步帧到达且本段已累计 ≥ 此值才切(I 帧间隔 =
    /// 段长秒,场景切额外同步帧不早切——段长稳定在 [此值, 段长+段长] 窗)。
    public static let segmentCutMinMs: Int64 = 2_000
}

/// 视频轨描述(init 段 moov 装配原料)。
public struct VideoTrackSpec: Sendable {
    public let width: Int
    public let height: Int
    /// AVCDecoderConfigurationRecord(avcC 载荷;管线面由 `avcCRecord
    /// (sps:pps:)` 自 VT format description 参数集装配——VT 无 csd-0
    /// Annex-B 形,Android 面的 csd 拆 NAL 转换在 iOS 即本函数)。
    public let avcC: [UInt8]
    public let fps: Int

    public init(width: Int, height: Int, avcC: [UInt8], fps: Int) {
        self.width = width
        self.height = height
        self.avcC = avcC
        self.fps = fps
    }
}

/// 音频轨描述(ASC = AudioSpecificConfig,即 AAC magic cookie;AAC-LC 假设)。
public struct AudioTrackSpec: Sendable {
    public let sampleRate: Int
    public let channels: Int
    public let asc: [UInt8]

    public init(sampleRate: Int, channels: Int, asc: [UInt8]) {
        self.sampleRate = sampleRate
        self.channels = channels
        self.asc = asc
    }
}

/// 目标尺寸:1080p 封顶(不放大),偶数对齐(编码器普遍约束)。纯函数
/// (宿主可测面——分段参数矩阵在此钉;Android targetDims 同构)。
public func targetDims(srcW: Int, srcH: Int) -> (width: Int, height: Int) {
    let h = min(max(srcH, 2), TranscodeParams.targetHeight)
    let w = max(Int((Double(srcW) * Double(h) / Double(max(srcH, 1))).rounded()), 2)
    return (w & ~1, h & ~1)
}

/// 段界规则(纯函数;宿主钉面,V4-1 T13 评审 Critical #1 同款移port):
/// 同步视频样到达时是否应「先收当前段、再入本样」——当前段**有样**且本样
/// 同步且本段已累计 ≥ minMs = true。段首样恒可入(hasSamples = false 恒
/// false:段 1 的首 IDR 开段;参数名直书「有样」免取反误传)。旧形「样入
/// 段后切」会把 IDR 留在前段尾、后段以 P 帧开段(引用前段 IDR)违反
/// fMP4/HLS 段独立——段界恒关键帧起。
public func cutBeforeSync(
    hasSamples: Bool, accumulatedMs: Int64, isSync: Bool, minMs: Int64,
) -> Bool {
    hasSamples && isSync && accumulatedMs >= minMs
}

/// 音频 ASC 种子(纯函数;宿主钉面,V4-1 T13 评审 Important #2 同款):
/// 直通臂 = 源 magic cookie 即 AAC ASC(容器原生,直接进 esds);**转编臂
/// 恒 nil**——源配置非 ASC 形(如 AC3/PCM 面),误种会错填 esds 且令
/// muxer 的「音频 csd 齐」等待条件假满足,真 ASC 出自 reader 输出样
/// format description 的 magic cookie(管线收)。
public func audioAscSeedFor(passthrough: Bool, sourceCsd: [UInt8]?) -> [UInt8]? {
    passthrough ? sourceCsd : nil
}

/// AVCDecoderConfigurationRecord 装配(纯函数;宿主钉面):VT format
/// description 参数集(SPS + PPS 表)→ avcC 记录载荷。首参 = SPS,余表 =
/// PPS(1+多 形,实机 format description 恒此形)。
public func avcCRecord(sps: [UInt8], ppss: [[UInt8]]) throws -> [UInt8] {
    guard !sps.isEmpty else {
        throw TranscodeFailure("SPS 缺席,avcC 记录不可装配")
    }
    let out = ByteWriter()
    out.u8(1) // configurationVersion
    out.u8(sps.count > 1 ? Int(sps[1]) : 0) // profile(SPS[1])
    out.u8(sps.count > 2 ? Int(sps[2]) : 0) // compatible(SPS[2])
    out.u8(sps.count > 3 ? Int(sps[3]) : 0) // level(SPS[3])
    out.u8(0xFF) // reserved(6)+lengthSizeMinusOne(3)
    out.u8(0xE0) // reserved(3)+numSPS(恒 1)
    out.u16(sps.count)
    out.bytes(sps)
    out.u8(ppss.count)
    for pps in ppss {
        out.u16(pps.count)
        out.bytes(pps)
    }
    return out.toByteArray()
}

/// Annex-B ↔ AVCC 转换单点(VT 输出 Annex-B 形的设备面:00 00 00 01 起始
/// 码分隔的 NAL 序列 → MP4/fMP4 面的 4 字节长度前缀;extensions 含 avcC 的
/// 设备面样恒长度前缀形,转换免)。大端手盒 muxer 的伴生面,宿主可测。
public enum AnnexB {

    private static let start3: [UInt8] = [0, 0, 1]
    private static let start4: [UInt8] = [0, 0, 0, 1]

    /// 起始码分隔的 NAL 序列 → NAL 载荷表(去起始码;空 NAL 跳)。
    public static func splitNals(_ data: [UInt8]) -> [[UInt8]] {
        var out: [[UInt8]] = []
        var nalStart = -1
        var i = 0
        while i < data.count {
            if matchAt(data, i, start4) {
                if nalStart >= 0 { out.append(Array(data[nalStart..<i])) }
                nalStart = i + 4
                i += 4
            } else if matchAt(data, i, start3) {
                if nalStart >= 0 { out.append(Array(data[nalStart..<i])) }
                nalStart = i + 3
                i += 3
            } else {
                i += 1
            }
        }
        if nalStart >= 0 && nalStart < data.count {
            out.append(Array(data[nalStart...]))
        }
        return out.filter { !$0.isEmpty }
    }

    private static func matchAt(_ data: [UInt8], _ at: Int, _ code: [UInt8]) -> Bool {
        guard at + code.count <= data.count else { return false }
        return (0..<code.count).allSatisfy { data[at + $0] == code[$0] }
    }

    /// 编码样(Annex-B NAL 序列)→ AVCC 样(逐 NAL 4 字节大端长度前缀;
    /// 与 avcC lengthSizeMinusOne=3 对齐)。
    public static func avccSample(from data: [UInt8]) -> [UInt8] {
        let nals = splitNals(data)
        let out = ByteWriter()
        for nal in nals {
            out.u32(nal.count)
            out.bytes(nal)
        }
        return out.toByteArray()
    }
}

/// muxer 装配契约违规(空段 finish 等;管线面如实抛错 → 响应器失败回告)。
public struct TranscodeFailure: Error {
    public let message: String
    public init(_ message: String) {
        self.message = message
    }
}

/// fMP4/CMAF 分段 muxer(纯字节装配;样恒 AVCC 形入 mux,见头注)。
public final class SegmentMuxer {

    /// 视频轨 track_ID(moov/mfhd 面;恒 1)。
    private let videoTrackId: Int = 1

    /// 音频轨 track_ID(恒 2;无音频轨不进 moof)。
    private let audioTrackId: Int = 2

    /// 视频轨 timescale(90k:30fps 帧长整数化无残差)。
    private let videoTimescale: Int = 90_000

    private let video: VideoTrackSpec
    private let audio: AudioTrackSpec?

    // ---------- 段积累状态(begin/finish 之间;单泵线程串行访问)----------

    private var fragSeq = 0
    private var videoBaseUs: Int64 = Int64.min
    private var audioBaseUs: Int64 = Int64.min
    private var videoSamples: [(ptsUs: Int64, keyFrame: Bool, data: [UInt8])] = []
    private var audioSamples: [(ptsUs: Int64, data: [UInt8])] = []

    public init(video: VideoTrackSpec, audio: AudioTrackSpec?) {
        self.video = video
        self.audio = audio
    }

    /// 开新内容段(seq = 段序,1 起与文件名 `{seq}.m4s` 对齐;0 = init)。
    /// 各轨 tfdt 基点自本段首样 pts 自动落定(调用方免管基点账)。
    public func beginFragment(seq: Int) {
        precondition(
            videoSamples.isEmpty && audioSamples.isEmpty,
            "上段未 finish")
        fragSeq = seq
        videoBaseUs = Int64.min
        audioBaseUs = Int64.min
    }

    /// 视频样入段(dts=pts 假设:基线编码序;非单调 pts 由管线拒)。
    /// data 恒 AVCC 长度前缀形(mdat 直拷面;Annex-B 形先经
    /// `AnnexB.avccSample` 转换——管线单点)。
    public func addVideoSample(ptsUs: Int64, keyFrame: Bool, data: [UInt8]) {
        if videoBaseUs == Int64.min { videoBaseUs = ptsUs }
        videoSamples.append((ptsUs, keyFrame, data))
    }

    /// 音频样入段(AAC 帧长恒 1024 样)。
    public func addAudioSample(ptsUs: Int64, data: [UInt8]) {
        if audioBaseUs == Int64.min { audioBaseUs = ptsUs }
        audioSamples.append((ptsUs, data))
    }

    /// 当前段是否已有样(段界判据面;段首样恒可入——评审 Critical #1)。
    public var fragmentHasSamples: Bool {
        !videoSamples.isEmpty || !audioSamples.isEmpty
    }

    /// 本段视频时长毫秒(末样时长按前样时长外推;无样 = 0)。
    public func videoAccumulatedMs() -> Int64 {
        guard let first = videoSamples.first, let last = videoSamples.last else {
            return 0
        }
        // 末样时长(ts)先回 µs 再累计(ts 与 µs 不可直加)
        let lastDurUs = tsToUs(Int64(videoDurationTs(videoSamples.count - 1)), videoTimescale)
        return ((last.ptsUs - first.ptsUs) + lastDurUs) / 1_000
    }

    /// 收段装配:`moof+mdat` 全字节(mfhd 序号 = `beginFragment` 的 seq)。
    public func finishFragment() throws -> [UInt8] {
        guard !videoSamples.isEmpty else {
            throw TranscodeFailure("空段不可出(视频段必有视频样)")
        }
        let audioSpec = audio // 音样在场 ⟹ 音轨在场(构造契约)
        // 逐样时长(pts 差分;末样按前差外推/单样按帧率)
        let vSizes = videoSamples.map { $0.data.count }
        let vDurTs = (0..<videoSamples.count).map { videoDurationTs($0) }
        let aDurTs = audioSamples.map { _ in 1024 }
        let videoBytes = videoSamples.reduce(0) { $0 + $1.data.count }
        let audioBytes = audioSamples.reduce(0) { $0 + $1.data.count }

        // trun 载荷尺寸 → traf/moof 总尺寸 → data-offset(mdat 样面自 moof
        // 起算,default-base-is-moof)。traf 盒含自有 8 字节头。
        let vTrunPayload = 12 + videoSamples.count * 12 // version/flags+count+offset + 样表
        let aTrunPayload = audioSamples.isEmpty ? 0 : 12 + audioSamples.count * 12
        let vTrafSize = 8 + 16 + 20 + 8 + vTrunPayload
        let aTrafSize = audioSamples.isEmpty ? 0 : 8 + 16 + 20 + 8 + aTrunPayload
        let moofSize = 8 + 16 + vTrafSize + aTrafSize
        let videoOffset = moofSize + 8 // mdat 头 8 字节后
        let audioOffset = videoOffset + videoBytes

        let out = ByteWriter()
        out.u32(moofSize) // moof 盒(独立盒子;mdat 随后自 declared 头)
        out.four("moof")
        // mfhd
        out.u32(16); out.four("mfhd"); out.u32(0); out.u32(fragSeq)
        // traf(video)
        out.u32(vTrafSize); out.four("traf")
        // tfhd:default-base-is-moof
        out.u32(16); out.four("tfhd"); out.u32(0x020000); out.u32(videoTrackId)
        // tfdt v1(基点 = 本段首样 pts 自动账)
        out.u32(20); out.four("tfdt"); out.u32(0x01000000)
        out.u64(usToTs(max(videoBaseUs, 0), videoTimescale))
        // trun v0:data-offset + duration/size/flags 表
        out.u32(8 + vTrunPayload); out.four("trun")
        out.u32(0x000701)
        out.u32(videoSamples.count)
        out.i32(videoOffset)
        for i in 0..<videoSamples.count {
            out.u32(vDurTs[i])
            out.u32(vSizes[i])
            out.u32(videoSamples[i].keyFrame ? 0x02000000 : 0x01010000)
        }
        // traf(audio)
        if !audioSamples.isEmpty {
            out.u32(aTrafSize); out.four("traf")
            out.u32(16); out.four("tfhd"); out.u32(0x020000); out.u32(audioTrackId)
            out.u32(20); out.four("tfdt"); out.u32(0x01000000)
            out.u64(usToTs(max(audioBaseUs, 0), audioSpec?.sampleRate ?? 48_000))
            out.u32(8 + aTrunPayload); out.four("trun")
            out.u32(0x000701)
            out.u32(audioSamples.count)
            out.i32(audioOffset)
            for i in 0..<audioSamples.count {
                out.u32(aDurTs[i])
                out.u32(audioSamples[i].data.count)
                out.u32(0x02000000) // 音频样恒 sync 面
            }
        }
        // mdat
        out.u32(8 + videoBytes + audioBytes)
        out.four("mdat")
        for s in videoSamples { out.bytes(s.data) }
        for s in audioSamples { out.bytes(s.data) }

        videoSamples.removeAll(keepingCapacity: true)
        audioSamples.removeAll(keepingCapacity: true)
        return out.toByteArray()
    }

    /// init 段全字节(`ftyp+moov`;一次成型,可重复取)。
    public func initSegment() -> [UInt8] {
        let moov = buildMoov()
        let out = ByteWriter()
        // ftyp:iso6(兼容 isom/iso6/avc1/mp41;MSE avc1+mp4a SourceBuffer 直吃)
        out.u32(8 + 8 + 16)
        out.four("ftyp")
        out.four("iso6")
        out.u32(0)
        for b in ["isom", "iso6", "avc1", "mp41"] { out.four(b) }
        out.bytes(moov)
        return out.toByteArray()
    }

    // ---------- moov 装配 ----------

    private func buildMoov() -> [UInt8] {
        let body = ByteWriter()
        body.bytes(mvhd())
        body.bytes(videoTrak(video))
        if let audio { body.bytes(audioTrak(audio)) }
        let out = ByteWriter()
        out.u32(8 + body.size)
        out.four("moov")
        out.bytes(body.toByteArray())
        return out.toByteArray()
    }

    private func mvhd() -> [UInt8] {
        let out = ByteWriter()
        out.u32(108) // 8 + 100(v0 定长载荷)
        out.four("mvhd")
        out.u32(0) // version 0 + flags 0
        out.u32(0); out.u32(0) // creation/modification
        out.u32(1000) // timescale
        out.u32(0) // duration(fragmented = 0)
        out.u32(0x00010000) // rate
        out.u16(0x0100) // volume
        out.u16(0); out.u64(0) // reserved
        // matrix(unity: 0x10000,0,0, 0,0x10000,0, 0,0,0x40000000)
        out.u32(0x00010000); out.u32(0); out.u32(0)
        out.u32(0); out.u32(0x00010000); out.u32(0)
        out.u32(0); out.u32(0); out.u32(0x40000000)
        out.u32(0); out.u32(0); out.u32(0)
        out.u32(0); out.u32(0); out.u32(0)
        out.u32(audio == nil ? 2 : 3) // next_track_ID
        return out.toByteArray()
    }

    private func tkhd(trackId: Int, volume: Int, width: Int, height: Int) -> [UInt8] {
        let out = ByteWriter()
        out.u32(92) // 8 + 84(v0 定长载荷)
        out.four("tkhd")
        out.u32(0x000007) // version 0 + flags(enabled|in_movie|in_preview)
        out.u32(0); out.u32(0)
        out.u32(trackId)
        out.u32(0) // reserved
        out.u32(0) // duration(fragmented = 0)
        out.u32(0); out.u32(0) // reserved
        out.u16(0) // layer
        out.u16(0) // alternate_group
        out.u16(volume)
        out.u16(0) // reserved
        out.u32(0x00010000); out.u32(0); out.u32(0)
        out.u32(0); out.u32(0x00010000); out.u32(0)
        out.u32(0); out.u32(0); out.u32(0x40000000)
        out.u32(width << 16)
        out.u32(height << 16)
        return out.toByteArray()
    }

    private func mdhd(timescale: Int) -> [UInt8] {
        let out = ByteWriter()
        out.u32(32) // 8 + 24
        out.four("mdhd")
        out.u32(0)
        out.u32(0); out.u32(0)
        out.u32(timescale)
        out.u32(0)
        out.u16(0x55C4) // "und"
        out.u16(0)
        return out.toByteArray()
    }

    private func hdlr(handler: String, name: String) -> [UInt8] {
        let out = ByteWriter()
        let nameBytes = Array((name + "\u{0000}").utf8)
        out.u32(8 + 24 + nameBytes.count)
        out.four("hdlr")
        out.u32(0)
        out.u32(0)
        out.four(handler)
        out.u32(0); out.u32(0); out.u32(0)
        out.bytes(nameBytes)
        return out.toByteArray()
    }

    private func dinf() -> [UInt8] {
        let out = ByteWriter()
        out.u32(28) // dref box = 8 + 20
        out.four("dref")
        out.u32(0)
        out.u32(1) // entry_count
        out.u32(12); out.four("url "); out.u32(1) // self-contained flags=1
        let dref = out.toByteArray()
        let w = ByteWriter()
        w.u32(8 + dref.count)
        w.four("dinf")
        w.bytes(dref)
        return w.toByteArray()
    }

    private func stbl(sampleEntry: [UInt8]) -> [UInt8] {
        let body = ByteWriter()
        // stsd
        body.u32(8 + 8 + sampleEntry.count)
        body.four("stsd")
        body.u32(0)
        body.u32(1)
        body.bytes(sampleEntry)
        // 空样表(fMP4:样在 moof/trun)
        body.u32(16); body.four("stts"); body.u32(0); body.u32(0)
        body.u32(16); body.four("stsc"); body.u32(0); body.u32(0)
        body.u32(20); body.four("stsz"); body.u32(0); body.u32(0); body.u32(0)
        body.u32(16); body.four("stco"); body.u32(0); body.u32(0)
        return wrap("stbl", body.toByteArray())
    }

    private func videoTrak(_ spec: VideoTrackSpec) -> [UInt8] {
        let mdiaBody = ByteWriter()
        mdiaBody.bytes(mdhd(timescale: videoTimescale))
        mdiaBody.bytes(hdlr(handler: "vide", name: "VideoHandler"))
        let minfBody = ByteWriter()
        // vmhd
        minfBody.u32(20); minfBody.four("vmhd"); minfBody.u32(1)
        minfBody.u16(0); minfBody.u16(0); minfBody.u16(0); minfBody.u16(0)
        minfBody.bytes(dinf())
        minfBody.bytes(stbl(sampleEntry: avc1Entry(spec)))
        mdiaBody.bytes(wrap("minf", minfBody.toByteArray()))
        let trakBody = ByteWriter()
        trakBody.bytes(tkhd(trackId: videoTrackId, volume: 0, width: spec.width, height: spec.height))
        trakBody.bytes(wrap("mdia", mdiaBody.toByteArray()))
        return wrap("trak", trakBody.toByteArray())
    }

    private func audioTrak(_ spec: AudioTrackSpec) -> [UInt8] {
        let mdiaBody = ByteWriter()
        mdiaBody.bytes(mdhd(timescale: spec.sampleRate))
        mdiaBody.bytes(hdlr(handler: "soun", name: "SoundHandler"))
        let minfBody = ByteWriter()
        // smhd
        minfBody.u32(16); minfBody.four("smhd"); minfBody.u32(0)
        minfBody.u16(0); minfBody.u16(0)
        minfBody.bytes(dinf())
        minfBody.bytes(stbl(sampleEntry: mp4aEntry(spec)))
        mdiaBody.bytes(wrap("minf", minfBody.toByteArray()))
        let trakBody = ByteWriter()
        trakBody.bytes(tkhd(trackId: audioTrackId, volume: 0x0100, width: 0, height: 0))
        trakBody.bytes(wrap("mdia", mdiaBody.toByteArray()))
        return wrap("trak", trakBody.toByteArray())
    }

    /// avc1 采样条目(avcC 记录 = 管线面 `avcCRecord` 产物;样面同轨:mdat
    /// 内 NAL 恒 4 字节长度前缀)。
    private func avc1Entry(_ spec: VideoTrackSpec) -> [UInt8] {
        let entry = ByteWriter()
        entry.bytes([UInt8](repeating: 0, count: 6)) // reserved
        entry.u16(1) // data_reference_index
        entry.u16(0); entry.u16(0); entry.bytes([UInt8](repeating: 0, count: 12)) // pre_defined/reserved
        entry.u16(spec.width)
        entry.u16(spec.height)
        entry.u32(0x00480000) // horizresolution 72dpi
        entry.u32(0x00480000) // vert
        entry.u32(0) // reserved
        entry.u16(1) // frame_count
        entry.bytes([UInt8](repeating: 0, count: 32)) // compressorname
        entry.u16(0x0018) // depth
        entry.u16(0xFFFF) // pre_defined
        let avccBox = ByteWriter()
        avccBox.u32(8 + spec.avcC.count)
        avccBox.four("avcC")
        avccBox.bytes(spec.avcC)
        entry.bytes(avccBox.toByteArray())
        return wrap("avc1", entry.toByteArray())
    }

    /// mp4a 采样条目(esds = AAC ASC 直装)。
    private func mp4aEntry(_ spec: AudioTrackSpec) -> [UInt8] {
        let entry = ByteWriter()
        entry.bytes([UInt8](repeating: 0, count: 6)) // reserved
        entry.u16(1) // data_reference_index
        entry.bytes([UInt8](repeating: 0, count: 8)) // reserved
        entry.u16(spec.channels)
        entry.u16(0x0010) // samplesize
        entry.u16(0) // pre_defined
        entry.u16(0) // reserved
        entry.u32(spec.sampleRate << 16)
        entry.bytes(esds(spec.asc))
        return wrap("mp4a", entry.toByteArray())
    }

    /// esds 盒子(描述符 TLV;AAC ASC 量级 < 127 = 短形长度)。
    private func esds(_ asc: [UInt8]) -> [UInt8] {
        func descriptor(_ tag: Int, _ content: [UInt8]) -> [UInt8] {
            let out = ByteWriter()
            out.u8(tag)
            out.u8(content.count) // 短形长度(ASC 场景恒 < 127)
            out.bytes(content)
            return out.toByteArray()
        }
        let decSpecific = descriptor(0x05, asc)
        let decConfigBody = ByteWriter()
        decConfigBody.u8(0x40) // objectTypeIndication = AAC
        decConfigBody.u8(0x15) // streamType = audio(0x05)<<2 | upstream=1
        decConfigBody.u24(0) // bufferSizeDB
        decConfigBody.u32(0) // maxBitrate
        decConfigBody.u32(0) // avgBitrate
        decConfigBody.bytes(decSpecific)
        let decConfig = descriptor(0x04, decConfigBody.toByteArray())
        let slConfig = descriptor(0x06, [0x02])
        let esBody = ByteWriter()
        esBody.u16(0); esBody.u8(0)
        esBody.bytes(decConfig)
        esBody.bytes(slConfig)
        let es = descriptor(0x03, esBody.toByteArray())
        let out = ByteWriter()
        out.u32(8 + 4 + es.count)
        out.four("esds")
        out.u32(0) // version/flags
        out.bytes(es)
        return out.toByteArray()
    }

    // ---------- 小件 ----------

    private func wrap(_ type: String, _ body: [UInt8]) -> [UInt8] {
        let out = ByteWriter()
        out.u32(8 + body.count)
        out.four(type)
        out.bytes(body)
        return out.toByteArray()
    }

    /// 视频样 i 的时长(timescale 单位;末样外推前差/单样按帧率)。
    private func videoDurationTs(_ i: Int) -> Int {
        let cur = videoSamples[i].ptsUs
        let durUs: Int64
        if i + 1 < videoSamples.count {
            durUs = videoSamples[i + 1].ptsUs - cur
        } else if videoSamples.count >= 2 {
            durUs = cur - videoSamples[i - 1].ptsUs
        } else {
            durUs = 1_000_000 / Int64(max(video.fps, 1))
        }
        return Int(usToTs(durUs, videoTimescale))
    }

    private func usToTs(_ us: Int64, _ timescale: Int) -> Int64 {
        (us * Int64(timescale) + 500_000) / 1_000_000
    }

    private func tsToUs(_ ts: Int64, _ timescale: Int) -> Int64 {
        ts * 1_000_000 / Int64(timescale)
    }
}

/// 大端字节装配器(段装配件;AnnexB/avcCRecord 共用;宿主可测)。
final class ByteWriter {
    private var buf: [UInt8] = []

    var size: Int { buf.count }

    func u8(_ v: Int) {
        buf.append(UInt8(v & 0xFF))
    }

    func u16(_ v: Int) {
        buf.append(UInt8((v >> 8) & 0xFF))
        buf.append(UInt8(v & 0xFF))
    }

    func u24(_ v: Int) {
        buf.append(UInt8((v >> 16) & 0xFF))
        buf.append(UInt8((v >> 8) & 0xFF))
        buf.append(UInt8(v & 0xFF))
    }

    func u32(_ v: Int) {
        buf.append(UInt8((v >> 24) & 0xFF))
        buf.append(UInt8((v >> 16) & 0xFF))
        buf.append(UInt8((v >> 8) & 0xFF))
        buf.append(UInt8(v & 0xFF))
    }

    func i32(_ v: Int) {
        u32(v)
    }

    func u64(_ v: Int64) {
        u32(Int(UInt64(bitPattern: v) >> 32))
        u32(Int(UInt64(bitPattern: v) & 0xFFFF_FFFF))
    }

    func four(_ s: String) {
        let b = Array(s.utf8)
        precondition(b.count == 4, "fourcc 须 4 字节: \(s)")
        buf.append(contentsOf: b)
    }

    func bytes(_ b: [UInt8]) {
        buf.append(contentsOf: b)
    }

    func toByteArray() -> [UInt8] {
        buf
    }
}
