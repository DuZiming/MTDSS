package com.mtdss.android.transcode

import android.media.Image
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaExtractor
import android.media.MediaFormat
import android.util.Log
import com.mtdss.android.serve.SegmentNotifier
import com.mtdss.android.serve.TranscoderFactory
import com.mtdss.android.send.OpenedSource
import java.io.File
import java.io.IOException
import java.nio.ByteBuffer

/**
 * 实机转码管线(V4-1 T13;MediaCodec 解→编,实机面——android.jar 桩无真
 * 解码,JVM 单测以注入伪工厂测协议,解码/编码正确性归实机冒烟):
 *
 * - **解→编**:视频轨 MediaCodec 解码 → 硬编 AVC(H.264,
 *   [TranscodeParams.TARGET_BITRATE_BPS];>1080p 源软降采样到
 *   [TranscodeParams.TARGET_HEIGHT] 封顶,不放大);音频轨 AAC **直通**
 *   (extractor 原始 AAC 帧直进 mux,免解免编)或非 AAC 转编 AAC-LC。
 * - **分段**:段长 [TranscodeParams.SEGMENT_MS](I 帧间隔对齐;同步帧到达
 *   且已累计 ≥ [TranscodeParams.SEGMENT_CUT_MIN_MS] 才切,场景切额外同步
 *   帧不早切);index 0 = init 段(T12 段序约定),内容段 1..N,文件名
 *   `{index}.m4s` 落 sessionDir,逐段经 [SegmentNotifier.onSegment] 回告
 *   (daemon 按需读已产段)。
 * - **start_at**:就近关键帧落地(MediaExtractor SEEK_TO_PREVIOUS_SYNC——
   起点段 = 前关键帧,seek =「起点段」重开会话语义,daemon 侧 open 承载)。
 * - **异常收口**:全管线 try/finally(codecs/extractor 恒释放;fd 所有权
 *   归调方——响应器在工厂返回后 release);编码器输出非单调 pts(设备强
 *   插 B 帧,dts=pts 混流模型不支持)如实抛错 → 响应器失败回告 → 查看端
 *   回退;csd 前/段未开的极端乱序样入暂存,init 段落地即按序冲。
 * - **取消**:isCancelled 逐循环轮询,非阻塞 dequeue 下即时退出(响应器
 *   按旗静默,无失败回告噪音)。
 */
object AndroidRenditionTranscoder : TranscoderFactory {
    private const val TAG = "RenditionTranscoder"

    override fun transcode(
        source: OpenedSource,
        displayName: String,
        startAtMs: Long,
        sessionDir: File,
        notifier: SegmentNotifier,
        isCancelled: () -> Boolean,
    ) {
        VideoTranscoder(source, displayName, startAtMs, sessionDir, notifier, isCancelled).run()
    }
}

/** 取消旗(管线内部退出信号;响应器按 isCancelled 闭包静默)。 */
internal class TranscodeCancelled : IOException("转码已取消")

/** 目标尺寸:1080p 封顶(不放大),偶数对齐(编码器普遍约束)。纯函数
 *  (JVM 可测面——分段参数矩阵在此钉)。 */
internal fun targetDims(srcW: Int, srcH: Int): Pair<Int, Int> {
    val h = minOf(srcH.coerceAtLeast(2), TranscodeParams.TARGET_HEIGHT)
    val w = Math.round(srcW.toDouble() * h / srcH.coerceAtLeast(1)).toInt().coerceAtLeast(2)
    return Pair(w and (Int.MAX_VALUE - 1), h and (Int.MAX_VALUE - 1))
}

/** extractor 单样(泵内部件;top-level——inner 类嵌套类面禁置)。 */
private class Sample(val ptsUs: Long, val sync: Boolean, val data: ByteArray)

internal class VideoTranscoder(
    private val source: OpenedSource,
    private val displayName: String,
    private val startAtMs: Long,
    private val sessionDir: File,
    private val notifier: SegmentNotifier,
    private val isCancelled: () -> Boolean,
) {
    private val tag = "VideoTranscoder"

    fun run() {
        val extractor = MediaExtractor()
        var videoDecoder: MediaCodec? = null
        var videoEncoder: MediaCodec? = null
        var audioDecoder: MediaCodec? = null
        var audioEncoder: MediaCodec? = null
        try {
            extractor.setDataSource(source.fd)
            // 轨面:首条视频轨 + 首条音频轨
            var videoIndex = -1
            var audioIndex = -1
            var videoFormat: MediaFormat? = null
            var audioFormat: MediaFormat? = null
            for (i in 0 until extractor.trackCount) {
                val f = extractor.getTrackFormat(i)
                val mime = f.getString(MediaFormat.KEY_MIME) ?: continue
                if (mime.startsWith("video/") && videoIndex < 0) {
                    videoIndex = i
                    videoFormat = f
                } else if (mime.startsWith("audio/") && audioIndex < 0) {
                    audioIndex = i
                    audioFormat = f
                }
            }
            val vFmt = videoFormat ?: throw IOException("无视频轨: $displayName")
            extractor.selectTrack(videoIndex)
            val hasAudio = audioIndex >= 0
            if (hasAudio) extractor.selectTrack(audioIndex)

            // 就近关键帧起点(seek =「起点段」重开会话语义)
            if (startAtMs > 0) {
                extractor.seekTo(startAtMs * 1000, MediaExtractor.SEEK_TO_PREVIOUS_SYNC)
                Log.i(tag, "起点段 seek: $displayName @$startAtMs ms")
            }

            // 视频解→编装配
            val decoder = MediaCodec.createDecoderByType(vFmt.getString(MediaFormat.KEY_MIME)!!)
            videoDecoder = decoder
            decoder.configure(vFmt, null, null, 0)
            decoder.start()
            val fps = if (vFmt.containsKey(MediaFormat.KEY_FRAME_RATE)) {
                vFmt.getInteger(MediaFormat.KEY_FRAME_RATE).coerceIn(1, 120)
            } else {
                30
            }
            val rotation = if (vFmt.containsKey("rotation")) vFmt.getInteger("rotation") else 0
            val dims = targetDims(vFmt.getInteger(MediaFormat.KEY_WIDTH), vFmt.getInteger(MediaFormat.KEY_HEIGHT))
            val encoder = createVideoEncoder(dims.first, dims.second, fps, rotation)
            videoEncoder = encoder
            encoder.start()

            // 音频面:AAC 直通(免解免编)/ 非 AAC 转编 AAC-LC
            val audioMime = audioFormat?.getString(MediaFormat.KEY_MIME)
            val passthrough = hasAudio && audioMime == "audio/mp4a-latm"
            var audioPair: Pair<MediaCodec, MediaCodec>? = null
            if (hasAudio && !passthrough) {
                val aDec = MediaCodec.createDecoderByType(audioMime!!)
                audioDecoder = aDec
                aDec.configure(audioFormat, null, null, 0)
                aDec.start()
                val aEnc = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC)
                audioEncoder = aEnc
                val encFmt = MediaFormat.createAudioFormat(
                    MediaFormat.MIMETYPE_AUDIO_AAC,
                    audioFormat.getInteger(MediaFormat.KEY_SAMPLE_RATE),
                    audioFormat.getInteger(MediaFormat.KEY_CHANNEL_COUNT),
                ).apply {
                    setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC)
                    setInteger(MediaFormat.KEY_BIT_RATE, TranscodeParams.AUDIO_BITRATE_BPS)
                    setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 65536)
                }
                aEnc.configure(encFmt, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                aEnc.start()
                audioPair = Pair(aDec, aEnc)
            }

            val pump = Pump(
                extractor = extractor,
                videoIndex = videoIndex,
                audioIndex = if (hasAudio) audioIndex else -1,
                decoder = decoder,
                encoder = encoder,
                encWidth = dims.first,
                encHeight = dims.second,
                fps = fps,
                audioPresent = hasAudio,
                audioPassthrough = passthrough,
                audioPair = audioPair,
                audioSampleRate = audioFormat?.getInteger(MediaFormat.KEY_SAMPLE_RATE) ?: 48000,
                audioChannels = audioFormat?.getInteger(MediaFormat.KEY_CHANNEL_COUNT) ?: 2,
                audioAscSeed = audioFormat?.getByteBuffer("csd-0")?.toByteArray(),
            )
            pump.run(sessionDir, notifier)
            Log.i(tag, "转码完成 $displayName(start@$startAtMs): ${pump.segmentsTotal} 段")
        } finally {
            // codecs/extractor 恒释放(fd 所有权归调方——响应器收尾 release)
            for (c in listOf(videoDecoder, videoEncoder, audioDecoder, audioEncoder)) {
                if (c != null) runCatching { runCatching { c.stop() }.onFailure { c.release() } }
            }
            runCatching { extractor.release() }
        }
    }

    /** 编码器装配:基线 profile 优先(B 帧免疫——dts=pts 混流假设),旋转
     *  元数据随源透传;设备拒配逐级退让(缺省 profile / 丢旋转,日志消费),
     *  全败如实抛错。 */
    internal fun createVideoEncoder(w: Int, h: Int, fps: Int, rotation: Int): MediaCodec {
        fun format(withProfile: Boolean, withRotation: Boolean): MediaFormat =
            MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h).apply {
                setInteger(
                    MediaFormat.KEY_COLOR_FORMAT,
                    MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible,
                )
                setInteger(MediaFormat.KEY_BIT_RATE, TranscodeParams.TARGET_BITRATE_BPS)
                setInteger(MediaFormat.KEY_FRAME_RATE, fps)
                setInteger(
                    MediaFormat.KEY_I_FRAME_INTERVAL,
                    (TranscodeParams.SEGMENT_MS / 1000).toInt().coerceAtLeast(2),
                )
                if (withProfile) {
                    setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
                }
                if (withRotation && rotation != 0) {
                    setInteger("rotation", rotation)
                }
            }
        var last: Exception? = null
        for (a in listOf(
            booleanArrayOf(true, true),
            booleanArrayOf(true, false),
            booleanArrayOf(false, true),
            booleanArrayOf(false, false),
        )) {
            val encoder = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            try {
                encoder.configure(format(a[0], a[1]), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                if (!a[0]) Log.w(tag, "编码器拒基线 profile,落缺省(非单调 pts 运行时拒)")
                if (!a[1] && rotation != 0) Log.w(tag, "编码器拒旋转元数据,输出方向退化(日志消费)")
                return encoder
            } catch (e: Exception) {
                last = e
                runCatching { encoder.release() }
            }
        }
        throw IOException("视频编码器装配失败: ${last?.message}")
    }

    // ── 主泵(单线程同步循环:extractor → 解码 → 编码 → 分段 mux)─────────

    private inner class Pump(
        private val extractor: MediaExtractor,
        private val videoIndex: Int,
        private val audioIndex: Int,
        private val decoder: MediaCodec,
        private val encoder: MediaCodec,
        private val encWidth: Int,
        private val encHeight: Int,
        private val fps: Int,
        private val audioPresent: Boolean,
        private val audioPassthrough: Boolean,
        private val audioPair: Pair<MediaCodec, MediaCodec>?,
        private val audioSampleRate: Int,
        private val audioChannels: Int,
        private val audioAscSeed: ByteArray?,
    ) {
        /** 累计段数(含 init;完成日志面)。 */
        var segmentsTotal = 0
            private set

        // 段装配状态
        private var muxer: SegmentMuxer? = null
        private var nextIndex = 1 // init=0 随 mux 建立即出;内容段 1..N
        private var videoCsd: ByteArray? = null
        private var audioCsd: ByteArray? = audioAscSeed
        // csd 未齐/段未开的极端乱序样暂存(init 段落地按序冲)
        private val pendingVideo = ArrayList<Triple<Long, Boolean, ByteArray>>()
        private val pendingAudio = ArrayList<Pair<Long, ByteArray>>()
        private var syncSeen = false // 本段内已见同步帧(切段判据一半)

        // 源面状态
        private var extractorEos = false
        private var heldVideo: Sample? = null
        private var heldAudio: Sample? = null
        private var decoderInEos = false
        private var audioDecoderInEos = false
        private var audioEncoderInEos = false

        fun run(sessionDir: File, notifier: SegmentNotifier) {
            var encoderOutEos = false
            while (!encoderOutEos) {
                if (isCancelled()) throw TranscodeCancelled()
                feedExtractor()
                if (extractorEos && heldVideo == null && !decoderInEos) queueVideoEos()
                if (extractorEos && audioPair != null && heldAudio == null && !audioDecoderInEos) {
                    queueAudioDecoderEos()
                }
                drainVideoDecoder()
                drainAudioDecoderToEncoder()
                drainAudioEncoder()
                encoderOutEos = drainVideoEncoder()
                maybeCreateMuxer(sessionDir, notifier)
                maybeCut(sessionDir, notifier)
            }
            // 收尾:残余冲刷 + 尾段(不足下限也出——残段保真)
            drainAudioDecoderToEncoder()
            drainAudioEncoder()
            maybeCreateMuxer(sessionDir, notifier)
            flushTail(sessionDir, notifier)
        }

        /** extractor 单样路由(视频 → 解码器;音频 → 暂存(直通)/ 音频解码
         *  器;编/解码器输入暂满 = 持样下轮,不推进——帧序保真)。 */
        private fun feedExtractor() {
            if (heldVideo != null && feedVideoDecoder(heldVideo!!)) heldVideo = null
            if (heldAudio != null && audioPair != null && feedAudioDecoder(heldAudio!!)) heldAudio = null
            if (extractorEos) return
            if (heldVideo != null || (heldAudio != null && audioPair != null)) return
            val idx = extractor.sampleTrackIndex
            if (idx < 0) {
                extractorEos = true
                return
            }
            val size = extractor.sampleSize
            if (size <= 0) {
                extractor.advance()
                return
            }
            val buf = ByteBuffer.allocate(size.toInt())
            val n = extractor.readSampleData(buf, 0)
            if (n <= 0) {
                extractorEos = true
                return
            }
            val data = ByteArray(n)
            buf.position(0)
            buf.get(data)
            val sample = Sample(
                extractor.sampleTime,
                extractor.sampleFlags and MediaExtractor.SAMPLE_FLAG_SYNC != 0,
                data,
            )
            when (idx) {
                videoIndex -> if (!feedVideoDecoder(sample)) {
                    heldVideo = sample
                    return // 持样:不推进
                }
                audioIndex -> if (audioPassthrough) {
                    pendingAudio.add(Pair(sample.ptsUs, sample.data))
                } else if (audioPair != null) {
                    if (!feedAudioDecoder(sample)) {
                        heldAudio = sample
                        return
                    }
                }
            }
            extractor.advance()
        }

        /** 样喂视频解码器(非阻塞;false = 输入满,持样下轮)。 */
        private fun feedVideoDecoder(sample: Sample): Boolean {
            val inIdx = decoder.dequeueInputBuffer(0)
            if (inIdx < 0) return false
            val inBuf = decoder.getInputBuffer(inIdx)!!
            inBuf.clear()
            inBuf.put(sample.data)
            val flags = if (sample.sync) MediaCodec.BUFFER_FLAG_SYNC_FRAME else 0
            decoder.queueInputBuffer(inIdx, 0, sample.data.size, sample.ptsUs, flags)
            return true
        }

        /** 样喂音频解码器(同上;音频样无 sync 面)。 */
        private fun feedAudioDecoder(sample: Sample): Boolean {
            val pair = audioPair ?: return false
            val inIdx = pair.first.dequeueInputBuffer(0)
            if (inIdx < 0) return false
            val inBuf = pair.first.getInputBuffer(inIdx)!!
            inBuf.clear()
            inBuf.put(sample.data)
            pair.first.queueInputBuffer(inIdx, 0, sample.data.size, sample.ptsUs, 0)
            return true
        }

        private fun queueVideoEos() {
            val inIdx = decoder.dequeueInputBuffer(10_000)
            if (inIdx >= 0) {
                decoder.queueInputBuffer(inIdx, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                decoderInEos = true
            }
        }

        private fun queueAudioDecoderEos() {
            val inIdx = audioPair!!.first.dequeueInputBuffer(10_000)
            if (inIdx >= 0) {
                audioPair.first.queueInputBuffer(inIdx, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                audioDecoderInEos = true
            }
        }

        /** 视频解码器出(Image)→ 编码器入(同尺寸直拷 / 软降采样)。 */
        private fun drainVideoDecoder() {
            val info = MediaCodec.BufferInfo()
            while (true) {
                val outIdx = decoder.dequeueOutputBuffer(info, 0)
                if (outIdx < 0) return // TRY_AGAIN / 格式变更(源 csd 无消费面)
                val eos = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                if (!eos) {
                    val image = decoder.getOutputImage(outIdx)
                    if (image != null) {
                        feedEncoderImage(image, info.presentationTimeUs)
                        image.close()
                    }
                }
                decoder.releaseOutputBuffer(outIdx, false)
                if (eos) {
                    // 解码器 EOS → 编码器 EOS 入队(尾样随 drain 收)
                    val inIdx = encoder.dequeueInputBuffer(10_000)
                    if (inIdx >= 0) {
                        encoder.queueInputBuffer(inIdx, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                    }
                    return
                }
            }
        }

        private fun feedEncoderImage(image: Image, ptsUs: Long) {
            val inIdx = encoder.dequeueInputBuffer(10_000)
            if (inIdx < 0) {
                // 编码器满:本帧弃(极端背压面;实机不达——解码/编码同速泵)
                Log.w(tag, "编码器输入暂满,弃帧 @$ptsUs")
                return
            }
            val inImage = encoder.getInputImage(inIdx)!!
            if (image.width == inImage.width && image.height == inImage.height) {
                copyImage(image, inImage)
            } else {
                scaleImage(image, inImage)
            }
            encoder.queueInputBuffer(inIdx, 0, 0, ptsUs, 0)
        }

        /** 视频编码器出 → 暂存 + 切段判据;返回 true = 编码器 EOS 已出。 */
        private fun drainVideoEncoder(): Boolean {
            val info = MediaCodec.BufferInfo()
            var lastPtsUs = -1L
            while (true) {
                val outIdx = encoder.dequeueOutputBuffer(info, 0)
                when {
                    outIdx == MediaCodec.INFO_TRY_AGAIN_LATER -> return false
                    outIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        videoCsd = encoder.outputFormat.getByteBuffer("csd-0")?.toByteArray()
                    }
                    outIdx < 0 -> return false
                    else -> {
                        val eos = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                        if (info.size > 0) {
                            val buf = encoder.getOutputBuffer(outIdx)!!
                            val data = ByteArray(info.size)
                            buf.position(info.offset)
                            buf.get(data)
                            val pts = info.presentationTimeUs
                            if (pts < lastPtsUs) {
                                throw IOException("编码器输出 pts 非单调(设备 B 帧;本管线不支持): $pts < $lastPtsUs")
                            }
                            lastPtsUs = pts
                            val sync = info.flags and MediaCodec.BUFFER_FLAG_SYNC_FRAME != 0
                            if (sync) syncSeen = true
                            pendingVideo.add(Triple(pts, sync, data))
                        }
                        encoder.releaseOutputBuffer(outIdx, false)
                        if (eos) return true
                    }
                }
            }
        }

        /** 音频转编:解码出 → 编码器入。 */
        private fun drainAudioDecoderToEncoder() {
            val pair = audioPair ?: return
            val info = MediaCodec.BufferInfo()
            while (true) {
                val outIdx = pair.first.dequeueOutputBuffer(info, 0)
                if (outIdx < 0) return // TRY_AGAIN / 格式变更 / EOS 标记除外
                val eos = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                if (info.size > 0 && !audioEncoderInEos) {
                    val buf = pair.first.getOutputBuffer(outIdx)!!
                    val data = ByteArray(info.size)
                    buf.position(info.offset)
                    buf.get(data)
                    val inIdx = pair.second.dequeueInputBuffer(10_000)
                    if (inIdx >= 0) {
                        val inBuf = pair.second.getInputBuffer(inIdx)!!
                        inBuf.clear()
                        inBuf.put(data)
                        pair.second.queueInputBuffer(inIdx, 0, data.size, info.presentationTimeUs, info.flags)
                    }
                }
                pair.first.releaseOutputBuffer(outIdx, false)
                if (eos) {
                    val inIdx = pair.second.dequeueInputBuffer(10_000)
                    if (inIdx >= 0) {
                        pair.second.queueInputBuffer(inIdx, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                        audioEncoderInEos = true
                    }
                    return
                }
            }
        }

        /** 音频转编:编码器出 → 暂存(AAC 帧 + ASC 面)。 */
        private fun drainAudioEncoder() {
            val pair = audioPair ?: return
            val info = MediaCodec.BufferInfo()
            while (true) {
                val outIdx = pair.second.dequeueOutputBuffer(info, 0)
                when {
                    outIdx == MediaCodec.INFO_TRY_AGAIN_LATER -> return
                    outIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        if (audioCsd == null) {
                            audioCsd = pair.second.outputFormat.getByteBuffer("csd-0")?.toByteArray()
                        }
                    }
                    outIdx < 0 -> return
                    else -> {
                        val eos = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                        if (info.size > 0) {
                            val buf = pair.second.getOutputBuffer(outIdx)!!
                            val data = ByteArray(info.size)
                            buf.position(info.offset)
                            buf.get(data)
                            pendingAudio.add(Pair(info.presentationTimeUs, data))
                        }
                        pair.second.releaseOutputBuffer(outIdx, false)
                        if (eos) return
                    }
                }
            }
        }

        /** mux 装配时机:视频 csd 齐 + (无转编轨或音频 csd 齐)→ init 段
         *  (index 0,T12 段序)落地 + 首内容段开段 + 暂存样按序冲。 */
        private fun maybeCreateMuxer(sessionDir: File, notifier: SegmentNotifier) {
            if (muxer != null) {
                flushPending()
                return
            }
            val vCsd = videoCsd ?: return
            if (audioPair != null && audioCsd == null) return // 转编轨 csd 未齐
            val audioTrack = if (audioPresent) {
                AudioTrackSpec(audioSampleRate, audioChannels, audioCsd ?: ByteArray(0))
            } else {
                null
            }
            val m = SegmentMuxer(VideoTrackSpec(encWidth, encHeight, vCsd, fps), audioTrack)
            writeSegment(sessionDir, 0, m.initSegment(), notifier)
            m.beginFragment(nextIndex)
            muxer = m
            flushPending()
        }

        /** 暂存样冲 mux(视频 pts 序 / 音频 pts 序各自保序;mdat 布局 =
         *  视频样后音频样,与 trun data-offset 装配一致)。 */
        private fun flushPending() {
            val m = muxer ?: return
            for (v in pendingVideo) m.addVideoSample(v.first, v.second, v.third)
            pendingVideo.clear()
            for (a in pendingAudio) m.addAudioSample(a.first, a.second)
            pendingAudio.clear()
        }

        /** 切段检查(同步帧已见 + 累计 ≥ 下限):收段 → 落盘回告 → 下段开段。 */
        private fun maybeCut(sessionDir: File, notifier: SegmentNotifier) {
            val m = muxer ?: return
            if (!syncSeen || m.videoAccumulatedMs() < TranscodeParams.SEGMENT_CUT_MIN_MS) return
            syncSeen = false
            val bytes = m.finishFragment()
            writeSegment(sessionDir, nextIndex, bytes, notifier)
            nextIndex += 1
            m.beginFragment(nextIndex)
        }

        /** 尾段 flush(EOF;无视频样不出)。 */
        private fun flushTail(sessionDir: File, notifier: SegmentNotifier) {
            val m = muxer ?: return
            if (m.videoAccumulatedMs() <= 0) return
            val bytes = m.finishFragment()
            writeSegment(sessionDir, nextIndex, bytes, notifier)
        }

        private fun writeSegment(sessionDir: File, index: Int, bytes: ByteArray, notifier: SegmentNotifier) {
            if (isCancelled()) throw TranscodeCancelled()
            val file = File(sessionDir, "$index.m4s")
            file.writeBytes(bytes)
            segmentsTotal += 1
            notifier.onSegment(index, file)
        }

        // ── YUV Image 复制/降采样(YUV_420_888;行/像素步长感知)──────────

        private fun copyImage(src: Image, dst: Image) {
            for (p in 0 until minOf(src.planes.size, dst.planes.size)) {
                val sub = if (p > 0) 1 else 0
                copyPlane(
                    src.planes[p],
                    dst.planes[p],
                    dst.width shr sub,
                    dst.height shr sub,
                )
            }
        }

        /** 平面逐行拷(行内按源像素步长取点;行间按各自 rowStride 落位)。
         *  平面维度 = 平面语义宽高(色度半宽半高;semi-planar 亦同——两色度
         *  平面同维,按 pixelStride 取点即 U/V 分离)。 */
        private fun copyPlane(src: Image.Plane, dst: Image.Plane, width: Int, height: Int) {
            val srcBuf = src.buffer
            val dstBuf = dst.buffer
            if (src.rowStride == dst.rowStride && src.pixelStride == 1 && dst.pixelStride == 1) {
                srcBuf.rewind()
                dstBuf.rewind()
                dstBuf.put(srcBuf)
                return
            }
            for (r in 0 until height) {
                dstBuf.position(r * dst.rowStride)
                for (x in 0 until width) {
                    dstBuf.put(srcBuf.get(r * src.rowStride + x * src.pixelStride))
                }
            }
            dstBuf.rewind()
        }

        /** 软降采样(area-average;仅 >1080p 源走此面)。 */
        private fun scaleImage(src: Image, dst: Image) {
            for (p in 0 until minOf(src.planes.size, dst.planes.size)) {
                val sub = if (p > 0) 1 else 0
                scalePlane(
                    src.planes[p],
                    src.width shr sub,
                    src.height shr sub,
                    dst.planes[p],
                    dst.width shr sub,
                    dst.height shr sub,
                )
            }
        }

        private fun scalePlane(
            src: Image.Plane,
            srcW: Int,
            srcH: Int,
            dst: Image.Plane,
            dstW: Int,
            dstH: Int,
        ) {
            val srcBuf = src.buffer
            val dstBuf = dst.buffer
            val pixelStride = src.pixelStride
            val rowStride = src.rowStride
            for (dy in 0 until dstH) {
                dstBuf.position(dy * dst.rowStride)
                val sy0 = dy * srcH / dstH
                val sy1 = (dy * srcH / dstH + srcH / dstH).coerceAtLeast(sy0 + 1)
                for (dx in 0 until dstW) {
                    val sx0 = dx * srcW / dstW
                    val sx1 = (dx * srcW / dstW + srcW / dstW).coerceAtLeast(sx0 + 1)
                    var acc = 0L
                    var n = 0L
                    var sy = sy0
                    while (sy < sy1 && sy < srcH) {
                        var sx = sx0
                        while (sx < sx1 && sx < srcW) {
                            acc += srcBuf.get(sy * rowStride + sx * pixelStride).toInt() and 0xFF
                            n++
                            sx++
                        }
                        sy++
                    }
                    dstBuf.put((acc / n).toByte())
                }
            }
            dstBuf.rewind()
        }
    }
}

/** ByteBuffer → 字节组(csd 面小件)。 */
internal fun ByteBuffer.toByteArray(): ByteArray {
    val out = ByteArray(remaining())
    get(out)
    return out
}
