package com.mtdss.android.transcode

/**
 * 转码形态参数(V4-1 T13;daemon 侧 `preview::rendition::RenditionConfig`
 * 同值锚——段长/高度/码率窗,改值须双侧同步,rendition.rs 常量断言测试
 * 钉面。码率窗 4–6 Mbps 内取中为实编值;段切下限防场景切同步帧早切)。
 */
object TranscodeParams {
    /** 段长毫秒(daemon RenditionConfig.SEGMENT_MS 同值;playlist
     *  TARGETDURATION=4 的原料)。 */
    const val SEGMENT_MS: Long = 4_000L

    /** 目标高度封顶(1080p;低分辨率源不放大)。 */
    const val TARGET_HEIGHT: Int = 1080

    /** 目标码率窗(daemon 同值锚;实编值取窗内常数)。 */
    const val BITRATE_MIN_BPS: Int = 4_000_000
    const val BITRATE_MAX_BPS: Int = 6_000_000

    /** 实编码率(窗内取中;V1 常量,实机冒烟可调)。 */
    const val TARGET_BITRATE_BPS: Int = 5_000_000

    /** 音频转码码率(AAC-LC;直通轨不适用)。 */
    const val AUDIO_BITRATE_BPS: Int = 128_000

    /** 切段下限毫秒:视频同步帧到达且本段已累计 ≥ 此值才切(I 帧间隔 =
     *  段长秒,场景切额外同步帧不早切——段长稳定在 [此值, 段长+段长] 窗)。 */
    const val SEGMENT_CUT_MIN_MS: Long = 2_000L
}

/** 视频轨描述(init 段 moov 装配原料)。 */
class VideoTrackSpec(
    val width: Int,
    val height: Int,
    /** MediaCodec csd-0 原样(Annex-B:00 00 00 01 起始码分隔的 SPS/PPS;
     *  avcC 记录由 [AnnexB.toAvcC] 在 init 装配时转换——MediaCodec AVC 面
     *  恒 Annex-B,MP4 面须长度前缀 AVCC,转换归本类单点)。 */
    val avcC: ByteArray,
    val fps: Int,
)

/**
 * Annex-B ↔ AVCC 转换单点(MediaCodec AVC 输出 = 00 00 00 01 起始码分隔
 * 的 NAL 序列;MP4/fMP4 的 avc1 样与 avcC 记录 = 4 字节长度前缀):csd-0
 * 拆 NAL 装 avcC 记录([toAvcC]),编码样逐 NAL 长度前缀化([toAvccSample])
 * ——大端手盒 muxer 的伴生面,JVM 可测(真解码面归实机冒烟)。
 */
internal object AnnexB {

    private val START3 = intArrayOf(0, 0, 1)

    private val START4 = intArrayOf(0, 0, 0, 1)

    /** 起始码分隔的 NAL 序列 → NAL 载荷表(去起始码;空 NAL 跳)。 */
    fun splitNals(data: ByteArray): List<ByteArray> {
        val out = ArrayList<ByteArray>()
        var i = 0
        var nalStart = -1
        while (i < data.size) {
            if (matchAt(data, i, START4)) {
                if (nalStart >= 0) out.add(data.copyOfRange(nalStart, i))
                nalStart = i + 4
                i += 4
            } else if (matchAt(data, i, START3)) {
                if (nalStart >= 0) out.add(data.copyOfRange(nalStart, i))
                nalStart = i + 3
                i += 3
            } else {
                i++
            }
        }
        if (nalStart >= 0 && nalStart < data.size) out.add(data.copyOfRange(nalStart, data.size))
        return out.filter { it.isNotEmpty() }
    }

    private fun matchAt(data: ByteArray, at: Int, code: IntArray): Boolean {
        if (at + code.size > data.size) return false
        for (k in code.indices) {
            if (data[at + k].toInt() != code[k]) return false
        }
        return true
    }

    /** csd-0(Annex-B SPS/PPS)→ AVCDecoderConfigurationRecord 载荷
     *  (首 NAL = SPS,余 = PPS;1+多 形,App 实机 csd 恒此形)。 */
    fun toAvcC(csd: ByteArray): ByteArray {
        val nals = splitNals(csd)
        require(nals.isNotEmpty()) { "csd 无 NAL(非 Annex-B 形)" }
        val sps = nals.first()
        val ppss = nals.drop(1)
        val out = ByteWriter()
        out.u8(1) // configurationVersion
        out.u8(if (sps.size > 1) sps[1].toInt() and 0xFF else 0) // profile(SPS[1])
        out.u8(if (sps.size > 2) sps[2].toInt() and 0xFF else 0) // compatible(SPS[2])
        out.u8(if (sps.size > 3) sps[3].toInt() and 0xFF else 0) // level(SPS[3])
        out.u8(0xFF) // reserved(6)+lengthSizeMinusOne(3)
        out.u8(0xE0) // reserved(3)+numSPS(恒 1——首 NAL = SPS,余归 PPS)
        out.u16(sps.size.toLong())
        out.bytes(sps)
        out.u8(ppss.size)
        for (pps in ppss) {
            out.u16(pps.size.toLong())
            out.bytes(pps)
        }
        return out.toByteArray()
    }

    /** 编码样(Annex-B NAL 序列)→ AVCC 样(逐 NAL 4 字节大端长度前缀;
     *  与 avcC lengthSizeMinusOne=3 对齐)。 */
    fun toAvccSample(data: ByteArray): ByteArray {
        val nals = splitNals(data)
        val out = ByteWriter()
        for (nal in nals) {
            out.u32(nal.size.toLong())
            out.bytes(nal)
        }
        return out.toByteArray()
    }
}

/** 音频轨描述(ASC = AudioSpecificConfig,即 csd-0;AAC-LC 假设)。 */
class AudioTrackSpec(
    val sampleRate: Int,
    val channels: Int,
    val asc: ByteArray,
)

/**
 * fMP4/CMAF 分段 muxer(V4-1 T13;纯字节装配,JVM 可测):init 段
 * (`ftyp+moov`,T12 段序约定 index 0)+ 内容段(`moof+mdat`,每段独立
 * 起头可解;视频 90000 timescale / 音频 = 采样率 timescale,tfdt v1,
 * trun v0 带 data-offset,`default-base-is-moof`)。样面:视频轨 AVC
 * (MediaCodec 输出 = 4 字节长度前缀 AVCC 形,与 avcC lengthSize=4 对齐,
 * mdat 直拷),音频轨 AAC(原始帧直拷)。
 *
 * 用法:构造 → [initSegment] 出 init 段 → [beginFragment]/[addVideoSample]/
 * [addAudioSample]/…/[finishFragment] 逐段循环(段边界 = 关键帧,由管线
 * 决定;本类只管装配)。**混段**:视频+音频样同段(单 moof 双 traf)——
 * T12 playlist 单 rendition 形(MAP + seg/{i}.m4s)直接可吃。
 */
class SegmentMuxer(private val video: VideoTrackSpec, private val audio: AudioTrackSpec?) {

    /** 视频轨 track_ID(moov/mfhd 面;恒 1)。 */
    private val videoTrackId = 1

    /** 音频轨 track_ID(恒 2;无音频轨不进 moof)。 */
    private val audioTrackId = 2

    /** 视频视频 timescale(90k:30fps 帧长整数化无残差)。 */
    private val videoTimescale = 90_000

    // ---------- 段积累状态(begin/finish 之间)----------

    private var fragSeq = 0
    private var videoBaseUs = 0L
    private var audioBaseUs = 0L
    private val videoSamples = ArrayList<VideoSample>()
    private val audioSamples = ArrayList<AudioSample>()

    private class VideoSample(val ptsUs: Long, val keyFrame: Boolean, val data: ByteArray)
    private class AudioSample(val ptsUs: Long, val data: ByteArray)

    /** 开新内容段(seq = 段序,1 起与文件名 `{seq}.m4s` 对齐;0 = init)。
     *  各轨 tfdt 基点自本段首样 pts 自动落定(调用方免管基点账)。 */
    fun beginFragment(seq: Int) {
        check(videoSamples.isEmpty() && audioSamples.isEmpty()) { "上段未 finish" }
        fragSeq = seq
        videoBaseUs = Long.MIN_VALUE
        audioBaseUs = Long.MIN_VALUE
    }

    /** 视频样入段(dts=pts 假设:基线编码序;非单调 pts 由管线拒)。
     *  MediaCodec 编码样 = Annex-B,此处即转 AVCC 长度前缀(mdat 直拷面)。 */
    fun addVideoSample(ptsUs: Long, keyFrame: Boolean, data: ByteArray) {
        if (videoBaseUs == Long.MIN_VALUE) videoBaseUs = ptsUs
        videoSamples.add(VideoSample(ptsUs, keyFrame, AnnexB.toAvccSample(data)))
    }

    /** 音频样入段(AAC 帧长恒 1024 样)。 */
    fun addAudioSample(ptsUs: Long, data: ByteArray) {
        if (audioBaseUs == Long.MIN_VALUE) audioBaseUs = ptsUs
        audioSamples.add(AudioSample(ptsUs, data))
    }

    /** 当前段是否已有样(段界判据面;段首样恒可入——评审 Critical #1)。 */
    val fragmentHasSamples: Boolean
        get() = videoSamples.isNotEmpty() || audioSamples.isNotEmpty()

    /** 本段视频时长毫秒(末样时长按前样时长外推;无样 = 0)。 */
    fun videoAccumulatedMs(): Long {
        if (videoSamples.isEmpty()) return 0L
        val first = videoSamples.first().ptsUs
        val lastDurTs = videoDurationTs(videoSamples.size - 1)
        val last = videoSamples.last().ptsUs
        return ((last - first) + tsToUs(lastDurTs, videoTimescale)) / 1000
    }

    /** 收段装配:`moof+mdat` 全字节(mfhd 序号 = [beginFragment] 的 seq)。 */
    fun finishFragment(): ByteArray {
        check(videoSamples.isNotEmpty()) { "空段不可出(视频段必有视频样)" }
        val audioSpec = audio // 音样在场 ⟹ 音轨在场(构造契约;局部捕获可空面)
        // 逐样时长(pts 差分;末样按前差外推/单样按帧率)
        val vSizes = videoSamples.map { it.data.size }
        val vDurTs = (0 until videoSamples.size).map { videoDurationTs(it) }
        val aDurTs = audioSamples.map { 1024L }
        val videoBytes = videoSamples.sumOf { it.data.size }
        val audioBytes = audioSamples.sumOf { it.data.size }

        // trun 载荷尺寸 → traf/moof 总尺寸 → data-offset(mdat 样面自 moof
        // 起算,default-base-is-moof)。traf 盒含自有 8 字节头。
        val vTrunPayload = 12 + videoSamples.size * 12 // version/flags+count+offset + 样表
        val aTrunPayload = if (audioSamples.isEmpty()) 0 else 12 + audioSamples.size * 12
        val vTrafSize = 8 + 16 + 20 + 8 + vTrunPayload
        val aTrafSize = if (audioSamples.isEmpty()) 0 else 8 + 16 + 20 + 8 + aTrunPayload
        val moofSize = 8 + 16 + vTrafSize + aTrafSize
        val videoOffset = moofSize + 8 // mdat 头 8 字节后
        val audioOffset = videoOffset + videoBytes

        val out = ByteWriter()
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
        out.u64(usToTs(videoBaseUs.coerceAtLeast(0), videoTimescale))
        // trun v0:data-offset + duration/size/flags 表
        out.u32(8 + vTrunPayload); out.four("trun")
        out.u32(0x000701)
        out.u32(videoSamples.size)
        out.i32(videoOffset)
        for (i in videoSamples.indices) {
            out.u32(vDurTs[i].toLong())
            out.u32(vSizes[i].toLong())
            out.u32(if (videoSamples[i].keyFrame) 0x02000000L else 0x01010000L)
        }
        // traf(audio)
        if (audioSamples.isNotEmpty()) {
            out.u32(aTrafSize); out.four("traf")
            out.u32(16); out.four("tfhd"); out.u32(0x020000); out.u32(audioTrackId)
            out.u32(20); out.four("tfdt"); out.u32(0x01000000)
            out.u64(usToTs(audioBaseUs.coerceAtLeast(0), audioSpec!!.sampleRate))
            out.u32(8 + aTrunPayload); out.four("trun")
            out.u32(0x000701)
            out.u32(audioSamples.size)
            out.i32(audioOffset)
            for (i in audioSamples.indices) {
                out.u32(aDurTs[i])
                out.u32(audioSamples[i].data.size.toLong())
                out.u32(0x02000000L) // 音频样恒 sync 面
            }
        }
        // mdat
        out.u32(8 + videoBytes + audioBytes)
        out.four("mdat")
        for (s in videoSamples) out.bytes(s.data)
        for (s in audioSamples) out.bytes(s.data)

        videoSamples.clear()
        audioSamples.clear()
        return out.toByteArray()
    }

    /** init 段全字节(`ftyp+moov`;一次成型,可重复取)。 */
    fun initSegment(): ByteArray {
        val moov = buildMoov()
        val out = ByteWriter()
        // ftyp:iso6(兼容 isom/iso6/avc1/mp41;MSE avc1+mp4a SourceBuffer 直吃)
        out.u32(8 + 8 + 16)
        out.four("ftyp")
        out.four("iso6")
        out.u32(0)
        for (b in listOf("isom", "iso6", "avc1", "mp41")) out.four(b)
        out.bytes(moov)
        return out.toByteArray()
    }

    // ---------- moov 装配 ----------

    private fun buildMoov(): ByteArray {
        val mvhd = mvhd()
        val videoTrak = videoTrak()
        val audioTrak = audio?.let { audioTrak(it) }
        val body = ByteWriter()
        body.bytes(mvhd)
        body.bytes(videoTrak)
        if (audioTrak != null) body.bytes(audioTrak)
        val out = ByteWriter()
        out.u32(8 + body.size())
        out.four("moov")
        out.bytes(body.toByteArray())
        return out.toByteArray()
    }

    private fun mvhd(): ByteArray {
        val out = ByteWriter()
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
        out.u32(0); out.u32(0); out.u32(0); out.u32(0); out.u32(0); out.u32(0) // pre_defined
        out.u32(3) // next_track_ID(audio 存在 = 3;无音频 = 2 也无害)
        return out.toByteArray()
    }

    private fun tkhd(trackId: Int, volume: Int, width: Int, height: Int): ByteArray {
        val out = ByteWriter()
        out.u32(92) // 8 + 84(v0 定长载荷)
        out.four("tkhd")
        out.u32(0x000007) // version 0 + flags(enabled|in_movie|in_preview)
        out.u32(0); out.u32(0)
        out.u32(trackId.toLong())
        out.u32(0) // reserved
        out.u32(0) // duration(fragmented = 0)
        out.u32(0); out.u32(0) // reserved
        out.u16(0) // layer
        out.u16(0) // alternate_group
        out.u16(volume.toLong())
        out.u16(0) // reserved
        out.u32(0x00010000); out.u32(0); out.u32(0)
        out.u32(0); out.u32(0x00010000); out.u32(0)
        out.u32(0); out.u32(0); out.u32(0x40000000)
        out.u32(width.toLong() shl 16)
        out.u32(height.toLong() shl 16)
        return out.toByteArray()
    }

    private fun mdhd(timescale: Int): ByteArray {
        val out = ByteWriter()
        out.u32(32) // 8 + 24
        out.four("mdhd")
        out.u32(0)
        out.u32(0); out.u32(0)
        out.u32(timescale.toLong())
        out.u32(0)
        out.u16(0x55C4) // "und"
        out.u16(0)
        return out.toByteArray()
    }

    private fun hdlr(handler: String, name: String): ByteArray {
        val out = ByteWriter()
        val nameBytes = (name + "\u0000").toByteArray(Charsets.US_ASCII)
        out.u32(8 + 24 + nameBytes.size)
        out.four("hdlr")
        out.u32(0)
        out.u32(0)
        out.four(handler)
        out.u32(0); out.u32(0); out.u32(0)
        out.bytes(nameBytes)
        return out.toByteArray()
    }

    private fun dinf(): ByteArray {
        val out = ByteWriter()
        out.u32(28) // dref box = 8 + 20
        out.four("dref")
        out.u32(0)
        out.u32(1) // entry_count
        out.u32(12); out.four("url "); out.u32(1) // self-contained flags=1
        val dref = out.toByteArray()
        val wrap = ByteWriter()
        wrap.u32(8 + dref.size)
        wrap.four("dinf")
        wrap.bytes(dref)
        return wrap.toByteArray()
    }

    private fun stbl(sampleEntry: ByteArray): ByteArray {
        val body = ByteWriter()
        // stsd
        body.u32(8 + 8 + sampleEntry.size)
        body.four("stsd")
        body.u32(0)
        body.u32(1)
        body.bytes(sampleEntry)
        // 空样表(fMP4:样在 moof/trun)
        body.u32(16); body.four("stts"); body.u32(0); body.u32(0)
        body.u32(16); body.four("stsc"); body.u32(0); body.u32(0)
        body.u32(20); body.four("stsz"); body.u32(0); body.u32(0); body.u32(0)
        body.u32(16); body.four("stco"); body.u32(0); body.u32(0)
        val stblBody = body.toByteArray()
        val out = ByteWriter()
        out.u32(8 + stblBody.size)
        out.four("stbl")
        out.bytes(stblBody)
        return out.toByteArray()
    }

    private fun videoTrak(spec: VideoTrackSpec = video): ByteArray {
        val mdiaBody = ByteWriter()
        mdiaBody.bytes(mdhd(videoTimescale))
        mdiaBody.bytes(hdlr("vide", "VideoHandler"))
        val minfBody = ByteWriter()
        // vmhd
        minfBody.u32(20); minfBody.four("vmhd"); minfBody.u32(1); minfBody.u16(0)
        minfBody.u16(0); minfBody.u16(0); minfBody.u16(0)
        minfBody.bytes(dinf())
        minfBody.bytes(stbl(avc1Entry(spec)))
        val minf = wrap("minf", minfBody.toByteArray())
        mdiaBody.bytes(minf)
        val mdia = wrap("mdia", mdiaBody.toByteArray())
        val trakBody = ByteWriter()
        trakBody.bytes(tkhd(videoTrackId, volume = 0, width = spec.width, height = spec.height))
        trakBody.bytes(mdia)
        return wrap("trak", trakBody.toByteArray())
    }

    private fun audioTrak(spec: AudioTrackSpec): ByteArray {
        val mdiaBody = ByteWriter()
        mdiaBody.bytes(mdhd(spec.sampleRate))
        mdiaBody.bytes(hdlr("soun", "SoundHandler"))
        val minfBody = ByteWriter()
        // smhd
        minfBody.u32(16); minfBody.four("smhd"); minfBody.u32(0); minfBody.u16(0); minfBody.u16(0)
        minfBody.bytes(dinf())
        minfBody.bytes(stbl(mp4aEntry(spec)))
        val minf = wrap("minf", minfBody.toByteArray())
        mdiaBody.bytes(minf)
        val mdia = wrap("mdia", mdiaBody.toByteArray())
        val trakBody = ByteWriter()
        trakBody.bytes(tkhd(audioTrackId, volume = 0x0100, width = 0, height = 0))
        trakBody.bytes(mdia)
        return wrap("trak", trakBody.toByteArray())
    }

    /** avc1 采样条目(avcC 记录 = [AnnexB.toAvcC](csd Annex-B → 长度前缀
     *  记录);样面同轨:mdat 内 NAL 恒 4 字节长度前缀)。 */
    private fun avc1Entry(spec: VideoTrackSpec): ByteArray {
        val avcC = ByteWriter()
        avcC.bytes(AnnexB.toAvcC(spec.avcC))
        val entry = ByteWriter()
        entry.bytes(ByteArray(6)) // reserved
        entry.u16(1) // data_reference_index
        entry.u16(0); entry.u16(0); entry.bytes(ByteArray(12)) // pre_defined/reserved
        entry.u16(spec.width.toLong())
        entry.u16(spec.height.toLong())
        entry.u32(0x00480000) // horizresolution 72dpi
        entry.u32(0x00480000) // vert
        entry.u32(0) // reserved
        entry.u16(1) // frame_count
        entry.bytes(ByteArray(32)) // compressorname
        entry.u16(0x0018) // depth
        entry.u16(0xFFFF) // pre_defined
        val avccBox = ByteWriter()
        avccBox.u32(8 + avcC.size())
        avccBox.four("avcC")
        avccBox.bytes(avcC.toByteArray())
        entry.bytes(avccBox.toByteArray())
        return wrap("avc1", entry.toByteArray())
    }

    /** mp4a 采样条目(esds = AAC-LC ASC 直装)。 */
    private fun mp4aEntry(spec: AudioTrackSpec): ByteArray {
        val entry = ByteWriter()
        entry.bytes(ByteArray(6)) // reserved
        entry.u16(1) // data_reference_index
        entry.bytes(ByteArray(8)) // reserved
        entry.u16(spec.channels.toLong())
        entry.u16(0x0010) // samplesize
        entry.u16(0) // pre_defined
        entry.u16(0) // reserved
        entry.u32(spec.sampleRate.toLong() shl 16)
        entry.bytes(esds(spec.asc))
        return wrap("mp4a", entry.toByteArray())
    }

    /** esds 盒子(描述符 TLV;AAC ASC 量级 < 127 = 短形长度)。 */
    private fun esds(asc: ByteArray): ByteArray {
        fun descriptor(tag: Int, content: ByteArray): ByteArray {
            val out = ByteWriter()
            out.u8(tag)
            out.u8(content.size) // 短形长度(ASC 场景恒 < 127)
            out.bytes(content)
            return out.toByteArray()
        }
        val decSpecific = descriptor(0x05, asc)
        val decConfig = descriptor(
            0x04,
            ByteWriter().apply {
                u8(0x40) // objectTypeIndication = AAC
                u8(0x15) // streamType = audio(0x05)<<2 | upstream=1
                u24(0) // bufferSizeDB
                u32(0) // maxBitrate
                u32(0) // avgBitrate
            }.toByteArray() + decSpecific,
        )
        val slConfig = descriptor(0x06, byteArrayOf(0x02))
        val es = descriptor(
            0x03,
            ByteWriter().apply { u16(0); u8(0) }.toByteArray() + decConfig + slConfig,
        )
        val out = ByteWriter()
        out.u32(8 + 4 + es.size)
        out.four("esds")
        out.u32(0) // version/flags
        out.bytes(es)
        return out.toByteArray()
    }

    // ---------- 小件 ----------

    private fun wrap(type: String, body: ByteArray): ByteArray {
        val out = ByteWriter()
        out.u32(8 + body.size)
        out.four(type)
        out.bytes(body)
        return out.toByteArray()
    }

    /** 视频样 i 的时长(timescale 单位;末样外推前差/单样按帧率)。 */
    private fun videoDurationTs(i: Int): Long {
        val cur = videoSamples[i].ptsUs
        val durUs = if (i + 1 < videoSamples.size) {
            videoSamples[i + 1].ptsUs - cur
        } else if (videoSamples.size >= 2) {
            cur - videoSamples[i - 1].ptsUs
        } else {
            1_000_000L / video.fps.coerceAtLeast(1)
        }
        return usToTs(durUs, videoTimescale)
    }

    private fun usToTs(us: Long, timescale: Int): Long = (us * timescale + 500_000) / 1_000_000

    private fun tsToUs(ts: Long, timescale: Int): Long = ts * 1_000_000 / timescale

}// SegmentMuxer

/** 大端字节装配器(段装配件;AnnexB 共用;JVM 可测)。 */
internal class ByteWriter {
    private val buf = java.io.ByteArrayOutputStream()

    fun size(): Int = buf.size()

    fun u8(v: Int) = buf.write(v and 0xFF)

    fun u16(v: Long) {
        buf.write((v.toInt() ushr 8) and 0xFF)
        buf.write(v.toInt() and 0xFF)
    }

    fun u16(v: Int) = u16(v.toLong())

    fun u24(v: Long) {
        buf.write((v.toInt() shr 16) and 0xFF)
        buf.write((v.toInt() shr 8) and 0xFF)
        buf.write(v.toInt() and 0xFF)
    }

    fun u24(v: Int) = u24(v.toLong())

    fun u32(v: Long) {
        buf.write((v.toInt() shr 24) and 0xFF)
        buf.write((v.toInt() shr 16) and 0xFF)
        buf.write((v.toInt() shr 8) and 0xFF)
        buf.write(v.toInt() and 0xFF)
    }

    fun u32(v: Int) = u32(v.toLong())

    fun i32(v: Long) = u32(v)

    fun i32(v: Int) = i32(v.toLong())

    fun u64(v: Long) {
        u32(v ushr 32)
        u32(v and 0xFFFFFFFFL)
    }

    fun four(s: String) {
        val b = s.toByteArray(Charsets.US_ASCII)
        check(b.size == 4) { "fourcc 须 4 字节: $s" }
        buf.write(b)
    }

    fun bytes(b: ByteArray) = buf.write(b)

    fun toByteArray(): ByteArray = buf.toByteArray()
}

/** 段界规则(纯函数;JVM 钉面,V4-1 T13 评审 Critical #1):同步视频样到
 *  达时是否应「先收当前段、再入本样」——当前段**有样**且本样同步且本段已
 *  累计 ≥ minMs = true。段首样恒可入(hasSamples = false 恒 false:段 1
 *  的首 IDR 开段;参数名直书「有样」免取反误传)。旧形「样入段后切」会把
 *  IDR 留在前段尾、后段以 P 帧开段(引用前段 IDR)违反 fMP4/HLS 段独立
 *  ——段界恒关键帧起。 */
internal fun cutBeforeSync(
    hasSamples: Boolean,
    accumulatedMs: Long,
    isSync: Boolean,
    minMs: Long,
): Boolean = hasSamples && isSync && accumulatedMs >= minMs

/** 音频 ASC 种子(纯函数;JVM 钉面,V4-1 T13 评审 Important #2):直通臂 =
 *  源 csd-0 即 AAC ASC(容器原生,直接进 esds);**转编臂恒 null**——源
 *  csd-0 非 ASC 形(如 AC3 的 dac3 盒),误种会错填 esds 且令 muxer 的
 *  「音频 csd 齐」等待条件假满足,真 ASC 出自编码器 format 变更(管线收)。 */
internal fun audioAscSeedFor(passthrough: Boolean, sourceCsd: ByteArray?): ByteArray? =
    if (passthrough) sourceCsd else null
