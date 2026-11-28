/// 实机转码管线(V4-1 T14;spec「AVAssetReader 解 → VideoToolbox 硬编 H.264
/// → CMAF/fMP4 分段」;Kit ``RenditionTranscoding`` 的 App 目标实现,Photos
/// /AVFoundation/VideoToolbox/AudioToolbox 为实机面,Kit/宿主测试不触碰本
/// 文件——解码/编码正确性归实机冒烟,Ruling E 同款绑定归 App):
///
/// - **读泵**:源 fd 经 `fcntl(F_GETPATH)` 回沙箱路径(IOSThumbEncoder 先例
///   ——fd 只读不消费,所有权随入参交出管线,终局必关)→ `AVURLAsset`。
///   视频轨 = `AVAssetReaderVideoCompositionOutput`(`AVMutableVideoComposition
///   (propertiesOf:)` 携 preferredTransform——**旋转烤入帧**,renderSize =
///   ``targetDims`` = 1080p 封顶等比不放大,GPU 合成缩放 = Android 软降
///   采样的 iOS 等价面)出 NV12 CVPixelBuffer。**start_at 就近关键帧** =
///   `AVSampleCursor` 自请求时刻回步 `sampleIsFullSync` 找前关键帧
///   (Android SEEK_TO_PREVIOUS_SYNC 同语义;cursor 不支持的轨回退请求
///   时刻,reader timeRange 承载)。
/// - **音频两臂**(iOS SDK 真相:AVAssetReaderAudioOutput 不在 iOS 面;
///   TrackOutput 音频 = 原样包(nil settings)或 LinearPCM 解压):
///   **直通臂** = AAC 源(ASBD mFormatID 判定)TrackOutput nil settings
///   出原始 AAC 包(免解免编;ASC 种子 = 源 magic cookie);**转编臂** =
///   非 AAC 源 TrackOutput LinearPCM 16-bit 交错解压 → AudioConverter
///   PCM→AAC-LC 编码(AudioToolbox;码率 128k)——Android 双 MediaCodec 音频
///   链的 iOS 等价面;装配失败如实抛错 → 拒答回退。
/// - **编泵**:`VTCompressionSession` 硬编 AVC(基线 profile 优先
///   AllowFrameReordering=false——B 帧免疫 dts=pts 假设,拒配逐级退让
///   main/缺省,全败如实抛错;码率 = 实编值 5 Mbps 窗内取中;I 帧间隔对齐
///   段长 4s)。单线程同步泵跑**专用串行 DispatchQueue**(async 顶层
///   `withCheckedThrowingContinuation` 桥接——阻塞等待不占协作线程池):
///   视频样逐帧 EncodeFrame → 输出回调(VT 内部队)提取样字节 + sync 旗
///   (`kCMSampleAttachmentKey_NotSync`/`DependsOnOthers`)+ pts 入加锁
///   pending 列表 + 信号量,泵逐帧等信号量后按序冲(1:1 输入/输出计数;
///   AllowFrameReordering=false 保输出序 = 输入序)。**格式变更面** = 首个
///   输出样的 format description:extensions 含 `avcC` ⟹ 样恒 AVCC(长度
///   前缀)直用、记录直取;不含 ⟹ 样恒 Annex-B,记录由
///   `CMVideoFormatDescriptionGetH264ParameterSetAtIndex`(SPS/PPS)经
///   ``avcCRecord(sps:pps:)`` 装配、样逐个 ``AnnexB.avccSample(from:)``
///   转换(VT 版本差异两侧都有先例,首样判一次)。编码输出 pts 非单调
///   (设备强插 B 帧,dts=pts 混流模型不支持)如实抛错 → 失败回告。
/// - **分段**:init 段(视频 csd + 音频 ASC 双齐即出,index 0 = T12 段序)
///   + 内容段(纯函数 ``cutBeforeSync`` 逐样判界:当前段有样+本样同步+
///   累计 ≥ 2s → **先** finish 段落盘回告、beginFragment、**再**入本同步样
///   ——段界恒关键帧起,T13 Critical #1 同款);EOF 尾段 flush(不足下限
///   也出——残段保真)。段文件 `{sessionDir}/{index}.m4s` 落盘后经
///   `notifier` 回告。
/// - **音频 ASC**(T13 Important #2 钉面:编码器输出为准):直通臂 = 源
///   magic cookie(``audioAscSeedFor`` 透传);转编臂 = AudioConverter 的
///   kAudioConverterCompressionMagicCookie(真编码器 ASC,**勿用源 csd
///   冒充**);采样率/声道数自源轨 format description 收。
/// - **音频持样面**(Android heldAudio 同构):超前视频面的包持样下轮,
///   不丢(copyNextSampleBuffer 消费面不可重读)。
/// - **异常收口**:全管线 defer(reader.cancelReading + CompleteFrames/
///   Invalidate + AudioConverterDispose + fd 关闭);continuation 恰一次
///   恢复(泵单出口);取消旗逐循环轮询(取消失能后 ≤ 单轮询点退出)。

import AVFoundation
import AudioToolbox
import CoreMedia
import MTDSSKit
import VideoToolbox

struct IOSRenditionTranscoder: RenditionTranscoding {

    func transcode(
        fd: Int32,
        displayName: String,
        startAtMs: Int64,
        sessionDir: String,
        notifier: @escaping @Sendable (Int, String) -> Void,
        isCancelled: @escaping @Sendable () -> Bool,
    ) async throws {
        // fd → 沙箱路径(F_GETPATH;失败 = 不可解面,fd 本侧关即拒)
        var buf = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        guard fcntl(fd, F_GETPATH, &buf) == 0 else {
            Darwin.close(fd)
            throw TranscodeFailure("源路径不可得: \(displayName)")
        }
        let path = String(cString: buf)
        // 装配面(async 上下文:轨道/读出器/编码器——重活与阻塞泵在专用队列)
        let pump: TranscodePump
        do {
            pump = try await TranscodePump(
                fd: fd, path: path, displayName: displayName, startAtMs: startAtMs,
                sessionDir: sessionDir, notifier: notifier, isCancelled: isCancelled)
        } catch {
            // 装配失败:fd 所有权随管线约定,此处收口不泄漏
            Darwin.close(fd)
            throw error
        }
        // 泵在专用串行队列上跑(async 桥接——阻塞等待不占协作线程池;
        // pump.run 内部 defer 全量收口,continuation 恰一次恢复)
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let queue = DispatchQueue(label: "mtdss.rendition.pump", qos: .userInitiated)
            queue.async {
                do {
                    try pump.run()
                    cont.resume()
                } catch {
                    cont.resume(throwing: error)
                }
            }
        }
    }
}

/// 同步泵(AVAssetReader 样流 → VTCompressionSession → muxer 分段;装配面
/// 在 async 上下文,run 在专用串行队列)。
final class TranscodePump: @unchecked Sendable {

    // 装配面
    private let fd: Int32
    private let sessionDir: String
    private let notifier: @Sendable (Int, String) -> Void
    private let isCancelled: @Sendable () -> Bool
    private let dims: (width: Int, height: Int)
    private let fps: Int
    private let reader: AVAssetReader
    private let videoOutput: AVAssetReaderVideoCompositionOutput
    private let audioOutput: AVAssetReaderTrackOutput?
    /// 音频转编器(转编臂 nil;直通臂免)。
    private let audioEncoder: AudioAACEncoder?
    /// 音频 ASC(直通臂 = 源 magic cookie;转编臂 = AudioConverter 真 ASC
    /// ——编码器输出为准,T13 Important #2;var = 源轨缺 cookie 的兜底面,
    /// 首个音频输出样 format description 补收)。
    private var audioAsc: [UInt8]?
    private let audioSampleRate: Int
    private let audioChannels: Int
    private let session: VTCompressionSession
    private let core: VTEncoderCore

    // mux 状态(泵线程串行)
    private var muxer: SegmentMuxer?
    private var nextIndex = 1 // init=0 随 mux 建立即出;内容段 1..N
    private var audioEof = false
    /// 超前视频面的持样队列(pts 升序;copyNextSampleBuffer 消费面不可重读
    /// ——Android heldAudio 同构,多包面列队不丢)。
    private var heldAudioList: [(ptsUs: Int64, data: [UInt8])] = []
    private var pendingVideo: [(ptsUs: Int64, sync: Bool, data: [UInt8])] = []
    private var pendingAudio: [(ptsUs: Int64, data: [UInt8])] = []

    init(
        fd: Int32, path: String, displayName: String, startAtMs: Int64,
        sessionDir: String,
        notifier: @escaping @Sendable (Int, String) -> Void,
        isCancelled: @escaping @Sendable () -> Bool,
    ) async throws {
        self.fd = fd
        self.sessionDir = sessionDir
        self.notifier = notifier
        self.isCancelled = isCancelled

        let asset = AVURLAsset(url: URL(fileURLWithPath: path))
        guard let videoTrack = try await asset.loadTracks(withMediaType: .video).first else {
            throw TranscodeFailure("无视频轨: \(displayName)")
        }
        let audioTrack = try await asset.loadTracks(withMediaType: .audio).first

        // 1080p 封顶(不放大):naturalSize 经 preferredTransform 转正
        // (合成面旋转烤入帧;Android「rotation 透传编码器」的 iOS 等价面)
        let natural = try await videoTrack.load(.naturalSize)
        let transform = try await videoTrack.load(.preferredTransform)
        let upright = natural.applying(transform)
        let nominalFps = try await videoTrack.load(.nominalFrameRate)
        dims = targetDims(
            srcW: Int(abs(upright.width).rounded()),
            srcH: Int(abs(upright.height).rounded()))
        fps = max(Int(nominalFps.rounded()), 1)

        // start_at 就近关键帧(AVSampleCursor 回步 sampleIsFullSync;
        // cursor 不支持的轨回退请求时刻,reader timeRange 承载)
        var start = CMTime(value: CMTimeValue(max(startAtMs, 0)), timescale: 1_000)
        if startAtMs > 0, let cursor = videoTrack.makeSampleCursor(
            presentationTimeStamp: start) {
            while !cursor.currentSampleSyncInfo.sampleIsFullSync.boolValue {
                if cursor.stepInPresentationOrder(byCount: -1) == 0 { break } // BOF
            }
            if cursor.currentSampleSyncInfo.sampleIsFullSync.boolValue {
                let t = cursor.presentationTimeStamp
                if t.isNumeric, t.seconds >= 0, t < start { start = t }
            }
        }

        // 读出器(视频合成输出 = 缩放/旋转面)
        let reader = try AVAssetReader(asset: asset)
        reader.timeRange = CMTimeRange(start: start, end: CMTime.positiveInfinity)
        let composition = try AVMutableVideoComposition(propertiesOf: asset)
        composition.renderSize = CGSize(width: CGFloat(dims.width), height: CGFloat(dims.height))
        let videoOutput = AVAssetReaderVideoCompositionOutput(
            videoTracks: [videoTrack],
            videoSettings: [
                kCVPixelBufferPixelFormatTypeKey as String:
                    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            ])
        videoOutput.videoComposition = composition
        guard reader.canAdd(videoOutput) else {
            throw TranscodeFailure("视频读出器不可挂: \(displayName)")
        }
        reader.add(videoOutput)

        // 音频两臂(iOS SDK 真相:无 AVAssetReaderAudioOutput;TrackOutput
        // = nil settings 原样包 / LinearPCM 解压 + AudioConverter 编 AAC)
        var audioOutput: AVAssetReaderTrackOutput?
        var audioEncoder: AudioAACEncoder?
        var audioAsc: [UInt8]?
        var audioSampleRate = 48_000
        var audioChannels = 2
        if let audioTrack {
            // 源 ASBD(直通判定 + 转编器入参面;采样率/声道 = mux esds 真值)
            guard let desc = try await audioTrack.load(.formatDescriptions).first,
                  let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc)?.pointee else {
                throw TranscodeFailure("音频格式不可得: \(displayName)")
            }
            audioSampleRate = max(Int(asbd.mSampleRate), 1)
            audioChannels = max(Int(asbd.mChannelsPerFrame), 1)
            let passthrough = asbd.mFormatID == kAudioFormatMPEG4AAC
            if passthrough {
                // 直通臂:原始 AAC 包直出(免解免编;ASC 种子 = 源 magic
                // cookie——audioAscSeedFor 纯函数钉面)
                var cookieSize = 0
                let sourceAsc: [UInt8]? = CMAudioFormatDescriptionGetMagicCookie(
                    desc, sizeOut: &cookieSize).map { cookie in
                        Array(UnsafeBufferPointer(
                            start: cookie.assumingMemoryBound(to: UInt8.self),
                            count: cookieSize))
                    }
                audioAsc = sanitizeAudioAsc(audioAscSeedFor(passthrough: true, sourceCsd: sourceAsc) ?? [])
                let output = AVAssetReaderTrackOutput(track: audioTrack, outputSettings: nil)
                guard reader.canAdd(output) else {
                    throw TranscodeFailure("音频读出器不可挂: \(displayName)")
                }
                reader.add(output)
                audioOutput = output
            } else {
                // 转编臂:LinearPCM 16-bit 交错解压 → AudioConverter 编
                // AAC-LC(**ASC = 转编器真 ASC,勿用源 csd 冒充**——T13
                // Important #2;源 csd 非 ASC 形如 AC3 dac3)
                let output = AVAssetReaderTrackOutput(
                    track: audioTrack,
                    outputSettings: [
                        AVFormatIDKey: kAudioFormatLinearPCM,
                        AVLinearPCMBitDepthKey: 16,
                        AVLinearPCMIsNonInterleaved: false,
                        AVLinearPCMIsFloatKey: false,
                        AVLinearPCMIsBigEndianKey: false,
                    ])
                guard reader.canAdd(output) else {
                    throw TranscodeFailure("音频读出器不可挂: \(displayName)")
                }
                reader.add(output)
                audioOutput = output
                let encoder = try AudioAACEncoder(
                    sourceASBD: asbd, bitrateBps: TranscodeParams.audioBitrateBps)
                // 编码器输出 ASC(转编臂真值;AudioConverter 面;field 修:
                // cookie 同为树形面时净化取真 DSI)
                audioAsc = sanitizeAudioAsc(encoder.magicCookie ?? [])
                audioEncoder = encoder
            }
        }
        self.audioOutput = audioOutput
        self.audioEncoder = audioEncoder
        self.audioAsc = audioAsc
        self.audioSampleRate = audioSampleRate
        self.audioChannels = audioChannels

        guard reader.startReading() else {
            throw TranscodeFailure(
                "读出器启动失败: \(reader.error.map { String(describing: $0) } ?? "?")")
        }
        self.reader = reader
        self.videoOutput = videoOutput

        // 编码器(基线优先 AllowFrameReordering=false——B 帧免疫;拒配逐级
        // 退让 main/缺省,全败如实抛错;码率/I 帧间隔 = TranscodeParams 锚)
        let core = VTEncoderCore()
        self.core = core
        guard let session = Self.createCompressionSession(
            width: dims.width, height: dims.height, fps: fps, core: core) else {
            throw TranscodeFailure("视频编码器装配失败: \(displayName)")
        }
        self.session = session
    }

    /// VTCompressionSession 装配阶梯(Baseline → Main → 缺省;
    /// AllowFrameReordering=false 恒置——输出序 = 输入序 + 无 B 帧)。
    private static func createCompressionSession(
        width: Int, height: Int, fps: Int, core: VTEncoderCore,
    ) -> VTCompressionSession? {
        let callback: VTCompressionOutputCallback = {
            refCon, _, status, _, sampleBuffer in
            guard let refCon else { return }
            let core = Unmanaged<VTEncoderCore>.fromOpaque(refCon).takeUnretainedValue()
            core.receive(status: status, sampleBuffer: sampleBuffer)
        }
        let refCon = Unmanaged.passUnretained(core).toOpaque()
        let profiles: [CFString?] = [
            kVTProfileLevel_H264_Baseline_AutoLevel,
            kVTProfileLevel_H264_Main_AutoLevel,
            nil,
        ]
        for profile in profiles {
            var session: VTCompressionSession?
            let status = VTCompressionSessionCreate(
                allocator: nil, width: Int32(width), height: Int32(height),
                codecType: kCMVideoCodecType_H264, encoderSpecification: nil,
                imageBufferAttributes: nil, compressedDataAllocator: nil,
                outputCallback: callback, refcon: refCon, compressionSessionOut: &session)
            guard status == noErr, let s = session else { continue }
            VTSessionSetProperty(
                s, key: kVTCompressionPropertyKey_AverageBitRate,
                value: NSNumber(value: TranscodeParams.targetBitrateBps))
            VTSessionSetProperty(
                s, key: kVTCompressionPropertyKey_ExpectedFrameRate,
                value: NSNumber(value: fps))
            VTSessionSetProperty(
                s, key: kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                value: NSNumber(value: Double(TranscodeParams.segmentMs) / 1000.0))
            VTSessionSetProperty(
                s, key: kVTCompressionPropertyKey_AllowFrameReordering,
                value: kCFBooleanFalse)
            VTSessionSetProperty(
                s, key: kVTCompressionPropertyKey_RealTime, value: kCFBooleanFalse)
            if let profile {
                VTSessionSetProperty(
                    s, key: kVTCompressionPropertyKey_ProfileLevel, value: profile)
            }
            if VTCompressionSessionPrepareToEncodeFrames(s) == noErr {
                return s
            }
            VTCompressionSessionInvalidate(s)
        }
        return nil
    }

    // MARK: 主泵(单线程同步循环;run 在专用串行队列)

    func run() throws {
        defer {
            // 全量收口(reader/编码器/转编器/fd 恒释放;EOF 与异常路径同面)
            reader.cancelReading()
            VTCompressionSessionCompleteFrames(
                session, untilPresentationTimeStamp: CMTime.invalid)
            VTCompressionSessionInvalidate(session)
            audioEncoder?.dispose()
            Darwin.close(fd)
        }
        var videoEof = false
        while !videoEof {
            if isCancelled() { throw TranscodeFailure("转码已取消") }
            guard let sb = videoOutput.copyNextSampleBuffer() else {
                videoEof = true
                break
            }
            let ptsUs = Self.ptsUs(of: sb)
            // 音频先行排空(pts ≤ 本视频帧;段内音频随当轮 flush 入段)
            try drainAudio(uptoUs: ptsUs)
            guard let pixelBuffer = CMSampleBufferGetImageBuffer(sb) else { continue }
            try encode(pixelBuffer: pixelBuffer, ptsUs: ptsUs)
        }
        try drainAudio(uptoUs: Int64.max)
        // EOF 收尾:CompleteFrames → 排空编码器尾(1:1 计数)→ 尾段
        let status = VTCompressionSessionCompleteFrames(
            session, untilPresentationTimeStamp: CMTime.invalid)
        guard status == noErr else {
            throw TranscodeFailure("编码器收尾失败: \(status)")
        }
        while core.needsDrain() {
            if let failure = core.failureWord {
                throw TranscodeFailure(failure)
            }
            core.waitOutput()
            absorbEncoderOutputs()
            try maybeCreateMuxer()
            try flushPending()
        }
        try flushTail()
    }

    private func encode(pixelBuffer: CVImageBuffer, ptsUs: Int64) throws {
        let status = VTCompressionSessionEncodeFrame(
            session,
            imageBuffer: pixelBuffer,
            presentationTimeStamp: CMTime(value: CMTimeValue(ptsUs), timescale: 1_000_000),
            duration: CMTime.invalid,
            frameProperties: nil,
            sourceFrameRefcon: nil,
            infoFlagsOut: nil)
        guard status == noErr else {
            throw TranscodeFailure("编码帧失败: \(status)")
        }
        core.expectInput()
        core.waitOutput()
        if let failure = core.failureWord {
            throw TranscodeFailure(failure)
        }
        absorbEncoderOutputs()
        try maybeCreateMuxer()
        try flushPending()
    }

    /// 编码器输出吸收(格式变更面在 VTEncoderCore 首样判一次;样入 pending
    /// 后统一过段界判据)。
    private func absorbEncoderOutputs() {
        for out in core.takePending() {
            pendingVideo.append(out)
        }
    }

    // MARK: 音频(直通臂原样包直进 mux;转编臂 PCM → AudioConverter → AAC)

    /// 音频排空到 uptoUs(超前视频面的包**持样下轮不丢**——Android
    /// heldAudio 同构;copyNextSampleBuffer 消费面不可重读)。
    private func drainAudio(uptoUs: Int64) throws {
        guard audioOutput != nil else { return }
        // 持样先判(上轮超前样;列队按序入段)
        while let first = heldAudioList.first, first.ptsUs <= uptoUs {
            pendingAudio.append(first)
            heldAudioList.removeFirst()
        }
        while !audioEof {
            if isCancelled() { throw TranscodeFailure("转码已取消") }
            guard let sb = audioOutput!.copyNextSampleBuffer() else {
                audioEof = true
                break
            }
            // ASC 兜底(源轨 format description 缺 cookie 的直通面:首个
            // 输出样 format description 补收——同一 cookie 源,样级;field
            // 修:cookie 树形面净化取真 DSI)
            if audioAsc == nil, let desc = CMSampleBufferGetFormatDescription(sb) {
                var cookieSize = 0
                if let cookie = CMAudioFormatDescriptionGetMagicCookie(desc, sizeOut: &cookieSize) {
                    audioAsc = sanitizeAudioAsc(Array(UnsafeBufferPointer(
                        start: cookie.assumingMemoryBound(to: UInt8.self), count: cookieSize)))
                }
            }
            guard let encoder = audioEncoder else {
                // 直通臂:**逐样切分**(TrackOutput 原样包面不保证 1 buffer
                // = 1 AAC 帧——批量缓冲按 NumSamples/GetSampleSize/
                // GetSampleTimeStamp 逐帧入 mux,trun 样表逐帧正确面,评审
                // Important #1;账不平/拷贝失败 = 弃样,勿以整缓冲一枚假
                // 帧污样表)。首样超前面 = 全缓冲超前(样级 pts 单调):
                // 列队即停读。
                guard let packets = audioSamplePackets(from: sb) else { continue }
                if let first = packets.first, first.ptsUs > uptoUs {
                    heldAudioList.append(contentsOf: packets)
                    break
                }
                var ahead = false
                for p in packets {
                    if p.ptsUs > uptoUs {
                        heldAudioList.append(p) // 超前:持样列队,不丢
                        ahead = true
                    } else {
                        pendingAudio.append(p)
                    }
                }
                if ahead { break }
                continue
            }
            // 转编臂:PCM 整缓冲喂转编器(AudioConverter 按流消费,批量
            // 缓冲无帧界语义);AAC 包 pts 由转编器逐包递推;超前面持样
            // 列队即停读——音频面超前视频面时防全轨预排水撑内存
            guard let pcm = wholeBufferData(from: sb) else { continue }
            let ptsUs = Self.ptsUs(of: sb)
            var ahead = false
            for out in try encoder.encode(pcm: pcm, ptsUs: ptsUs) {
                if out.ptsUs > uptoUs {
                    heldAudioList.append(out) // 超前:持样列队,不丢
                    ahead = true
                    continue
                }
                pendingAudio.append((out.ptsUs, out.data))
            }
            if ahead { break }
        }
    }

    // MARK: mux 装配(段界状态机;Android Pump 同构)

    /// mux 装配时机:视频 csd 齐 + (无音频轨或音频 ASC 齐)→ init 段
    /// (index 0,T12 段序)落地 + 首内容段开段 + 暂存样按序冲。
    private func maybeCreateMuxer() throws {
        if muxer != nil {
            try flushPending()
            return
        }
        guard let vCsd = core.avcCData else { return }
        if audioOutput != nil && audioAsc == nil { return } // 音频 ASC 未齐
        let audioSpec: AudioTrackSpec?
        if audioOutput != nil {
            audioSpec = AudioTrackSpec(
                sampleRate: audioSampleRate,
                channels: audioChannels,
                asc: audioAsc ?? [])
        } else {
            audioSpec = nil
        }
        let m = SegmentMuxer(
            video: VideoTrackSpec(width: dims.width, height: dims.height, avcC: vCsd, fps: fps),
            audio: audioSpec)
        try writeSegment(index: 0, bytes: m.initSegment())
        nextIndex = 1
        m.beginFragment(seq: nextIndex)
        muxer = m
        try flushPending()
    }

    /// 暂存样冲 mux(视频 pts 序 / 音频 pts 序各自保序;mdat 布局 = 视频样
    /// 后音频样,与 trun data-offset 装配一致)。**段界规则单点**
    /// (`cutBeforeSync` 逐样判界):同步样到达且当前段已累计 ≥ 下限 →
    /// **先收当前段落盘回告、开新段,再入本同步样**——段界恒关键帧起
    /// (T13 评审 Critical #1 同款修)。
    private func flushPending() throws {
        guard let m = muxer else { return }
        for v in pendingVideo {
            if cutBeforeSync(
                hasSamples: m.fragmentHasSamples,
                accumulatedMs: m.videoAccumulatedMs(),
                isSync: v.sync,
                minMs: TranscodeParams.segmentCutMinMs) {
                try writeSegment(index: nextIndex, bytes: try m.finishFragment())
                nextIndex += 1
                m.beginFragment(seq: nextIndex)
            }
            m.addVideoSample(ptsUs: v.ptsUs, keyFrame: v.sync, data: v.data)
        }
        pendingVideo.removeAll(keepingCapacity: true)
        for a in pendingAudio {
            m.addAudioSample(ptsUs: a.ptsUs, data: a.data)
        }
        pendingAudio.removeAll(keepingCapacity: true)
    }

    /// 尾段 flush(EOF;无视频样不出——残段保真,不足下限也出)。
    private func flushTail() throws {
        guard let m = muxer else { return }
        guard m.videoAccumulatedMs() > 0 else { return }
        let bytes = try m.finishFragment()
        try writeSegment(index: nextIndex, bytes: bytes)
    }

    private func writeSegment(index: Int, bytes: [UInt8]) throws {
        if isCancelled() { throw TranscodeFailure("转码已取消") }
        let path = "\(sessionDir)/\(index).m4s"
        try Data(bytes).write(to: URL(fileURLWithPath: path))
        notifier(index, path)
    }

    // MARK: 小件

    private static func ptsUs(of sb: CMSampleBuffer) -> Int64 {
        Int64((CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sb)) * 1_000_000).rounded())
    }

}

// MARK: - 音频转编器(AudioConverter PCM → AAC-LC;Android 双 MediaCodec
// 音频链的 iOS 等价面;实机面——宿主不可触 AudioConverter 真编码)

/// PCM→AAC 转编器(AudioToolbox;AudioConverterFillComplexBuffer 同步泵面
/// ——filler 回调在 FillComplexBuffer 调用方线程同步执行,PCM 待编队列与
/// 泵线程同域,零锁面)。
final class AudioAACEncoder: @unchecked Sendable {

    /// PCM 待编面(泵线程写、filler 读——AudioConverterFillComplexBuffer
    /// 在泵线程同步调 filler,同域免锁)。
    private final class PCMFeed {
        private let bytesPerFrame: Int
        private var chunks: [(ptsUs: Int64, data: [UInt8])] = []
        private var offset = 0
        /// 首个被消费帧的 pts(pts 递推锚;一次锚定)。
        var basePtsUs: Int64?

        init(bytesPerFrame: Int) {
            self.bytesPerFrame = max(bytesPerFrame, 1)
        }

        func append(ptsUs: Int64, data: [UInt8]) {
            chunks.append((ptsUs, data))
        }

        /// filler 回调:填入 PCM(16-bit 交错;1 packet = 1 frame;上限
        /// 32 KiB/轮)。返回 0 包 = 暂无输入(排水面)。
        func fill(
            into ioData: UnsafeMutablePointer<AudioBufferList>,
            packetsOut: UnsafeMutablePointer<UInt32>,
        ) -> OSStatus {
            let abl = ioData.pointee
            guard abl.mNumberBuffers >= 1, let dst = abl.mBuffers.mData else {
                packetsOut.pointee = 0
                return noErr
            }
            var written = 0
            while !chunks.isEmpty {
                let chunk = chunks[0]
                if basePtsUs == nil { basePtsUs = chunk.ptsUs }
                let available = chunk.data.count - offset
                let room = 32 * 1024 - written
                if available == 0 || room <= 0 { break }
                let take = min(available, room)
                chunk.data.withUnsafeBytes { raw in
                    memcpy(dst + written, raw.baseAddress! + offset, take)
                }
                written += take
                offset += take
                if offset >= chunk.data.count {
                    chunks.removeFirst()
                    offset = 0
                }
            }
            var buffer = abl.mBuffers
            buffer.mDataByteSize = UInt32(written)
            ioData.pointee.mBuffers = buffer
            packetsOut.pointee = UInt32(written / bytesPerFrame)
            return noErr
        }
    }

    private var converter: AudioConverterRef?
    private let dstASBD: AudioStreamBasicDescription
    private let sampleRate: Int
    private var feed: PCMFeed!
    private var nextOutputPtsUs: Int64?
    /// 已输出的 AAC 帧数(pts 递推分子)。
    private var framesOut: Int64 = 0
    private let filler: AudioConverterComplexInputDataProc = {
        _, ioNumberOfDataPackets, ioData, _, userData in
        guard let userData else { return -1 }
        let feed = Unmanaged<PCMFeed>.fromOpaque(userData).takeUnretainedValue()
        return feed.fill(into: ioData, packetsOut: ioNumberOfDataPackets)
    }

    init(sourceASBD: AudioStreamBasicDescription, bitrateBps: Int) throws {
        var src = sourceASBD
        var dst = AudioStreamBasicDescription(
            mSampleRate: sourceASBD.mSampleRate,
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 0,
            mBytesPerFrame: 0,
            mChannelsPerFrame: sourceASBD.mChannelsPerFrame,
            mBitsPerChannel: 0,
            mReserved: 0)
        var converter: AudioConverterRef?
        let status = AudioConverterNew(&src, &dst, &converter)
        guard status == noErr, let c = converter else {
            throw TranscodeFailure("音频转编器装配失败: \(status)")
        }
        self.converter = c
        var bitrate = UInt32(bitrateBps)
        AudioConverterSetProperty(
            c, kAudioConverterEncodeBitRate, UInt32(MemoryLayout<UInt32>.size), &bitrate)
        dstASBD = dst
        sampleRate = max(Int(sourceASBD.mSampleRate), 1)
        // 交错 PCM 帧 = channels × 16-bit(ASBD mBytesPerFrame 同值;缺省自算)
        let bpf = sourceASBD.mBytesPerFrame > 0
            ? Int(sourceASBD.mBytesPerFrame)
            : Int(sourceASBD.mChannelsPerFrame) * 2
        feed = PCMFeed(bytesPerFrame: bpf)
    }

    /// 编码器输出 ASC(kAudioConverterCompressionMagicCookie——真编码器
    /// 面值,转编臂 esds 直装;构造即得)。
    var magicCookie: [UInt8]? {
        guard let c = converter else { return nil }
        // GetProperty Swift 面 out buffer 非可选——512B 面 ASC 直取
        // (AAC ASC 实际 ≤ 16B,size 回程 = 实长)
        var size: UInt32 = 512
        var bytes = [UInt8](repeating: 0, count: 512)
        let result = bytes.withUnsafeMutableBytes { raw in
            AudioConverterGetProperty(
                c, kAudioConverterCompressionMagicCookie, &size, raw.baseAddress!)
        }
        guard result == noErr, size > 0, Int(size) <= bytes.count else { return nil }
        return Array(bytes[..<Int(size)])
    }

    /// PCM 面入转编器,出 AAC 包(pts 由首消费帧锚定逐包递推)。
    func encode(pcm: [UInt8], ptsUs: Int64) throws -> [(ptsUs: Int64, data: [UInt8])] {
        guard let c = converter else { return [] }
        feed.append(ptsUs: ptsUs, data: pcm)
        guard feed.basePtsUs != nil else { return [] }
        // 输出面:单 buffer 大容量 + 包描述表(AAC 包 ~ 数百字节;512 包
        // 上限覆盖 512*1024 帧 ≈ 10s@48k,音频批用量级富余)
        let packetCapacity = 512
        let bufferBytes = 512 * 1024
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: bufferBytes, alignment: 16)
        defer { buffer.deallocate() }
        let descriptions = UnsafeMutablePointer<AudioStreamPacketDescription>
            .allocate(capacity: packetCapacity)
        defer { descriptions.deallocate() }
        var abl = AudioBufferList(
            mNumberBuffers: 1,
            mBuffers: AudioBuffer(
                mNumberChannels: UInt32(dstASBD.mChannelsPerFrame),
                mDataByteSize: UInt32(bufferBytes),
                mData: buffer))
        // Swift 6 参面:ioOutputDataPacketSize 回程 = 实产包数
        // (outNumberOfOutputPackets 不在 iOS Swift import 面)
        var produced: UInt32 = UInt32(packetCapacity)
        let status = AudioConverterFillComplexBuffer(
            c, filler, Unmanaged.passUnretained(feed).toOpaque(),
            &produced, &abl, descriptions)
        guard status == noErr else {
            throw TranscodeFailure("音频转编失败: \(status)")
        }
        var out: [(ptsUs: Int64, data: [UInt8])] = []
        if nextOutputPtsUs == nil, let base = feed.basePtsUs {
            nextOutputPtsUs = base
        }
        for i in 0..<Int(produced) {
            let desc = descriptions[i]
            let count = Int(desc.mDataByteSize)
            guard count > 0 else { continue }
            let frames = Int(desc.mVariableFramesInPacket) > 0
                ? Int(desc.mVariableFramesInPacket)
                : Int(max(dstASBD.mFramesPerPacket, 1024))
            let pts = nextOutputPtsUs ?? feed.basePtsUs ?? 0
            let data = [UInt8](UnsafeBufferPointer(
                start: buffer.assumingMemoryBound(to: UInt8.self) + Int(desc.mStartOffset),
                count: count))
            out.append((pts, data))
            framesOut += Int64(frames)
            nextOutputPtsUs = ptsFor(frames: framesOut)
        }
        return out
    }

    private func ptsFor(frames: Int64) -> Int64 {
        Int64((Double(frames) / Double(sampleRate) * 1_000_000.0).rounded())
    }

    /// 收口(AudioConverterDispose;幂等面)。
    func dispose() {
        if let c = converter {
            AudioConverterDispose(c)
            converter = nil
        }
    }
}

// MARK: - 视频编码器输出侧(VT 回调 → 泵线程的单点桥)

/// 编码器输出侧(回调线程 → 泵线程的单点桥;加锁 pending + 信号量;
/// 1:1 输入/输出计数面——AllowFrameReordering=false 保输出序 = 输入序)。
final class VTEncoderCore: @unchecked Sendable {

    private let lock = NSLock()
    private let semaphore = DispatchSemaphore(value: 0)
    private var pending: [(ptsUs: Int64, sync: Bool, data: [UInt8])] = []
    private var lastPtsUs: Int64?
    private var inputs = 0
    private var outputs = 0
    private var failure: String?
    // 格式变更单次态(avcC 记录 + Annex-B 旗;形态判 = extensions avcC 键
    // 优先,缺席时**字节面嗅探**(field 修:macOS 26/iOS 26 VT 无 avcC 键
    // 但样恒 AVCC——此前恒按 Annex-B 处理把全帧字节归零))
    private(set) var avcCData: [UInt8]?
    private var annexB = false
    private var annexBDecided = false

    /// 泵面:预告一次成功输入(EncodeFrame 成功后;计数面——失败帧不占
    /// 计数,防收尾排水挂死)。
    func expectInput() {
        lock.lock(); inputs += 1; lock.unlock()
    }

    /// 泵面:还有未吸收的输出(收尾排水循环判据;计数读在锁内)。
    func needsDrain() -> Bool {
        lock.lock(); defer { lock.unlock() }
        return outputs < inputs
    }

    /// 泵面:等一个输出(信号量;失败面也即发即收)。
    func waitOutput() {
        semaphore.wait()
    }

    /// 泵面:失败词面(锁内读;非 nil 即抛)。
    var failureWord: String? {
        lock.lock(); defer { lock.unlock() }
        return failure
    }

    /// 回调面:格式变更判据 + 样字节提取 + sync 旗 + 非单调 pts 守卫。
    func receive(status: OSStatus, sampleBuffer: CMSampleBuffer?) {
        guard status == noErr, let sb = sampleBuffer else {
            lock.lock()
            if failure == nil { failure = "编码器输出异常: \(status)" }
            lock.unlock()
            semaphore.signal()
            return
        }
        do {
            let sample = try Self.extract(from: sb, state: self)
            lock.lock()
            if let last = lastPtsUs, sample.ptsUs < last {
                if failure == nil {
                    failure = "编码器输出 pts 非单调(设备 B 帧;本管线不支持): "
                        + "\(sample.ptsUs) < \(last)"
                }
            } else {
                pending.append(sample)
                lastPtsUs = sample.ptsUs
                outputs += 1
            }
            lock.unlock()
        } catch {
            lock.lock()
            if failure == nil { failure = "\(error)" }
            lock.unlock()
        }
        semaphore.signal()
    }

    /// 泵面:取走全部 pending(通常恰本帧一枚)。
    func takePending() -> [(ptsUs: Int64, sync: Bool, data: [UInt8])] {
        lock.lock()
        let out = pending
        pending = []
        lock.unlock()
        return out
    }

    /// 样提取:format description → avcC(ext 直取或 SPS/PPS 装配)+ Annex-B
    /// 判定(首样判一次);样字节(AVCC 化)+ sync 旗 + pts。
    private static func extract(
        from sb: CMSampleBuffer, state: VTEncoderCore,
    ) throws -> (ptsUs: Int64, sync: Bool, data: [UInt8]) {
        // 格式变更面(首样判一次;VT 版本差异两侧都有先例)
        if state.avcCData == nil, let desc = CMSampleBufferGetFormatDescription(sb) {
            if let ext = CMFormatDescriptionGetExtensions(desc) as? [String: Any],
               let avccData = ext["avcC"] as? Data {
                state.avcCData = [UInt8](avccData)
                state.annexB = false // 样恒长度前缀形
                state.annexBDecided = true
            } else {
                var count = 0
                guard CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    desc, parameterSetIndex: 0, parameterSetPointerOut: nil,
                    parameterSetSizeOut: nil, parameterSetCountOut: &count,
                    nalUnitHeaderLengthOut: nil) == noErr, count > 0 else {
                    throw TranscodeFailure("视频参数集不可得(格式变更面)")
                }
                var sps: [UInt8] = []
                var ppss: [[UInt8]] = []
                for i in 0..<count {
                    var ptr: UnsafePointer<UInt8>?
                    var size = 0
                    guard CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        desc, parameterSetIndex: i, parameterSetPointerOut: &ptr,
                        parameterSetSizeOut: &size, parameterSetCountOut: nil,
                        nalUnitHeaderLengthOut: nil) == noErr, let p = ptr else {
                        throw TranscodeFailure("视频参数集 \(i) 不可得")
                    }
                    let bytes = Array(UnsafeBufferPointer(start: p, count: size))
                    if i == 0 { sps = bytes } else { ppss.append(bytes) }
                }
                state.avcCData = try avcCRecord(sps: sps, ppss: ppss)
                // 样形态**不在此判**:extensions 缺 avcC 不必然 Annex-B——
                // 首样字节面嗅探(下方 annexBDecided 门)
            }
        }
        // 样字节
        guard let bb = CMSampleBufferGetDataBuffer(sb) else {
            throw TranscodeFailure("编码样数据面缺席")
        }
        let length = CMBlockBufferGetDataLength(bb)
        guard length > 0 else {
            throw TranscodeFailure("编码样空载荷")
        }
        var bytes = [UInt8](repeating: 0, count: length)
        let copyStatus = bytes.withUnsafeMutableBytes { raw in
            CMBlockBufferCopyDataBytes(
                bb, atOffset: 0, dataLength: length, destination: raw.baseAddress!)
        }
        guard copyStatus == kCMBlockBufferNoErr else {
            throw TranscodeFailure("编码样拷贝失败: \(copyStatus)")
        }
        if !state.annexBDecided {
            // field 修:字节面形态嗅探(extensions 缺 avcC 的设备面,样恒
            // AVCC——此前恒 Annex-B 转换把全帧字节归零 = 视频样全空根因)
            state.annexBDecided = true
            state.annexB = sampleLooksAnnexB(bytes)
        }
        if state.annexB {
            bytes = AnnexB.avccSample(from: bytes)
        }
        // sync 旗(附件表 NotSync/DependsOnOthers;缺附件 = 默认同步样)
        var sync = true
        if let atts = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false)
            as? [[String: Any]], let first = atts.first {
            if let notSync = first[kCMSampleAttachmentKey_NotSync as String] as? Bool {
                sync = !notSync
            }
            if let depends = first[kCMSampleAttachmentKey_DependsOnOthers as String] as? Bool, depends {
                sync = false
            }
        }
        return (Self.ptsUs(of: sb), sync, bytes)
    }

    private static func ptsUs(of sb: CMSampleBuffer) -> Int64 {
        Int64((CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sb)) * 1_000_000).rounded())
    }
}
