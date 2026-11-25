package com.mtdss.android.transcode

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.ByteBuffer

/**
 * fMP4/CMAF 分段 muxer JVM 单测(V4-1 T13;纯字节装配面——真解码/编码面
 * 归实机冒烟,MediaCodec 无 JVM 档):init 段盒子结构(ftyp 品牌/moov 双
 * 轨/avcC 记录/esds ASC)、内容段 moof/mdat 装配(tfdt 基点/trun 样表/
 * mdat 载荷)、分段参数(TranscodeParams 与 daemon RenditionConfig 同值锚
 * + 1080p 封顶缩放 + 切段下限)、Annex-B → AVCC 转换(csd 拆 NAL 装记录/
 * 样面长度前缀化)。
 */
class SegmentMuxerTest {

    // ── 盒子遍历小件(测试专用;大端手解)─────────────────────────────────

    private class Box(val type: String, val body: ByteArray) {
        fun u32(off: Int): Long = ByteBuffer.wrap(body, off, 4).int.toLong() and 0xFFFFFFFFL

        fun u64(off: Int): Long {
            val hi = u32(off)
            val lo = u32(off + 4)
            return (hi shl 32) or lo
        }

        fun int(off: Int): Int = ByteBuffer.wrap(body, off, 4).int

        fun fourcc(off: Int): String = String(body.copyOfRange(off, off + 4), Charsets.US_ASCII)
    }

    private fun boxes(body: ByteArray): List<Box> {
        val bb = ByteBuffer.wrap(body)
        val out = ArrayList<Box>()
        while (bb.remaining() >= 8) {
            val size = bb.int.toLong() and 0xFFFFFFFFL
            assertEquals("盒子 size 须 ≥ 8(测试面受控字节)", true, size >= 8)
            val type = String(ByteArray(4).also { bb.get(it) }, Charsets.US_ASCII)
            val b = ByteArray((size - 8).toInt())
            bb.get(b)
            out.add(Box(type, b))
        }
        assertEquals("盒子遍历须恰尽", 0, bb.remaining())
        return out
    }

    private fun box(type: String, body: ByteArray): Box = boxes(body).single { it.type == type }

    private fun muxer(): SegmentMuxer = SegmentMuxer(
        VideoTrackSpec(
            width = 1920,
            height = 1080,
            // MediaCodec csd-0 原样面(Annex-B):SPS(4 载荷)+ 00 00 00 01 + PPS(5 载荷)
            avcC = byteArrayOf(
                0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28,
                0, 0, 0, 1, 0x68, 0xEB.toByte(), 0xEC.toByte(), 0xB2.toByte(), 0x2C.toByte(),
            ),
            fps = 30,
        ),
        AudioTrackSpec(sampleRate = 48000, channels = 2, asc = byteArrayOf(0x12, 0x10)),
    )

    /** 假编码样(Annex-B:起始码分隔两 NAL)。 */
    private fun annexBSample(): ByteArray = byteArrayOf(
        0, 0, 0, 1, 0x41, 0x9A.toByte(), 0x2B.toByte(),
        0, 0, 0, 1, 0x41, 0x17, 0x08,
    )

    // ── init 段 ───────────────────────────────────────────────────────────

    @Test
    fun `init 段盒子结构 ftyp 品牌 moov 双轨 avcC 记录与 esds ASC`() {
        val init = muxer().initSegment()
        val top = boxes(init)
        assertEquals(listOf("ftyp", "moov"), top.map { it.type })

        // ftyp:iso6 + 兼容面(isom/iso6/avc1/mp41)
        val ftyp = top[0]
        assertEquals(24, ftyp.body.size) // major(4)+minor(4)+4×brand
        assertEquals("iso6", ftyp.fourcc(0))
        assertEquals(0L, ftyp.u32(4))
        assertEquals("isom", ftyp.fourcc(8))
        assertEquals("iso6", ftyp.fourcc(12))
        assertEquals("avc1", ftyp.fourcc(16))
        assertEquals("mp41", ftyp.fourcc(20))

        // moov:mvhd + 2 trak + mvex(field 修:fMP4 必需)
        val moov = top[1]
        val kids = boxes(moov.body)
        assertEquals(listOf("mvhd", "trak", "trak", "mvex"), kids.map { it.type })
        assertEquals(100, kids[0].body.size) // mvhd 载荷(v0 定长)

        // mvex:双轨双 trex(无 trex 解析器拒段;默认值 0 + 缺省样描述 1)
        val trexes = boxes(kids[3].body)
        assertEquals(listOf("trex", "trex"), trexes.map { it.type })
        trexes.forEachIndexed { i, trex ->
            assertEquals(24, trex.body.size)
            assertEquals((i + 1).toLong(), trex.u32(0)) // track_ID
            assertEquals(1L, trex.u32(4)) // default_sample_description_index
            assertEquals(0L, trex.u32(8)) // default_sample_duration
            assertEquals(0L, trex.u32(12)) // default_sample_size
            assertEquals(0L, trex.u32(16)) // default_sample_flags
        }

        // 视频 trak:tkhd track_ID=1;width/height 16.16 = 1920/1080
        val vTkhd = box("tkhd", kids[1].body)
        assertEquals(84, vTkhd.body.size)
        assertEquals(1L, vTkhd.u32(12)) // track_ID
        assertEquals(1920L shl 16, vTkhd.u32(76)) // width 16.16
        assertEquals(1080L shl 16, vTkhd.u32(80)) // height 16.16

        // 视频轨 avcC 记录(csd Annex-B → 长度前缀记录)
        val avcC = avccOf(kids[1])
        val expectedAvcC = byteArrayOf(
            1, // configurationVersion
            0x64, 0x00, 0x28, // profile/compatible/level(取自 SPS[1..3])
            0xFF.toByte(), // lengthSizeMinusOne = 3
            0xE1.toByte(), // numSPS = 1(field 修:0xE0 位值=0「零参数集」)
            0, 4, 0x67, 0x64, 0x00, 0x28, // SPS
            1, // numPPS
            0, 5, 0x68, 0xEB.toByte(), 0xEC.toByte(), 0xB2.toByte(), 0x2C.toByte(), // PPS
        )
        assertEquals(expectedAvcC.toList(), avcC.toList())

        // 音频 trak:esds 含 ASC(csd 直装)+ objectTypeIndication = AAC
        val esds = esdsOf(kids[2])
        val asc = byteArrayOf(0x12, 0x10)
        assertTrue("esds 载荷含 ASC", esds.toList().windowed(asc.size).any { it == asc.toList() })
        assertEquals("esds objectTypeIndication = AAC(0x40)", 0x40, esds[11].toInt() and 0xFF)
    }

    /** trak → mdia → minf → stbl → stsd 深走,取 avcC 记录(stsd 条目自
     *  version/flags+entry_count 之后起盒;avc1 采样条目 78 字节固定头后
     *  才是子盒面)。 */
    private fun avccOf(trak: Box): ByteArray {
        val mdia = box("mdia", trak.body)
        val minf = box("minf", mdia.body)
        val stbl = box("stbl", minf.body)
        val stsd = box("stsd", stbl.body)
        val entry = boxes(stsd.body.copyOfRange(8, stsd.body.size)).single()
        assertEquals("avc1", entry.type)
        return box("avcC", entry.body.copyOfRange(78, entry.body.size)).body
    }

    /** trak → … → stsd,取 mp4a 的 esds 载荷(mp4a 采样条目 28 字节固定头
     *  后才是 esds 子盒)。 */
    private fun esdsOf(trak: Box): ByteArray {
        val mdia = box("mdia", trak.body)
        val minf = box("minf", mdia.body)
        val stbl = box("stbl", minf.body)
        val stsd = box("stsd", stbl.body)
        val entry = boxes(stsd.body.copyOfRange(8, stsd.body.size)).single()
        assertEquals("mp4a", entry.type)
        return box("esds", entry.body.copyOfRange(28, entry.body.size)).body
    }

    // ── 内容段 ────────────────────────────────────────────────────────────

    @Test
    fun `内容段 moof mdat 装配 tfdt 基点 trun 样表与 mdat 载荷`() {
        val m = muxer()
        m.beginFragment(3)
        m.addVideoSample(900_000, keyFrame = true, annexBSample())
        m.addVideoSample(930_000, keyFrame = false, annexBSample())
        m.addAudioSample(900_333, byteArrayOf(1, 2, 3))
        m.addAudioSample(902_390, byteArrayOf(4, 5, 6))

        val seg = m.finishFragment()
        val top = boxes(seg)
        assertEquals(listOf("moof", "mdat"), top.map { it.type })

        // moof → mfhd(seq=3)+ traf(v)+ traf(a)
        val kids = boxes(top[0].body)
        assertEquals(listOf("mfhd", "traf", "traf"), kids.map { it.type })
        assertEquals(3L, kids[0].u32(4))

        // 视频 traf:tfhd track 1 + tfdt v1(基点 = 首样 pts 900000us → 81000)
        val vTraf = kids[1]
        assertEquals(0x020000, box("tfhd", vTraf.body).int(0)) // default-base-is-moof
        assertEquals(1L, box("tfhd", vTraf.body).u32(4))
        val vTfdt = box("tfdt", vTraf.body)
        assertEquals(1, vTfdt.int(0) ushr 24) // version 1
        assertEquals(81000L, vTfdt.u64(4))
        val vTrun = box("trun", vTraf.body)
        assertEquals(0x000701, vTrun.int(0)) // data-offset+dur+size+flags
        assertEquals(2L, vTrun.u32(4)) // 样数

        // 音频 traf:tfhd track 2 + tfdt 基点 = 900333us → 43216(48000Hz)
        val aTraf = kids[2]
        assertEquals(2L, box("tfhd", aTraf.body).u32(4))
        assertEquals(43216L, box("tfdt", aTraf.body).u64(4))

        // mdat:视频样 AVCC 长度前缀(Annex-B 已转)+ 音频样直拷
        val mdat = top[1]
        val v1 = AnnexB.toAvccSample(annexBSample()) // 2×(4 长度+NAL)
        assertEquals(
            "mdat = 2 视频样(AVCC)+ 2 音频样",
            (v1.size * 2 + 3 + 3).toLong(),
            mdat.body.size.toLong(),
        )
        assertEquals(
            "AVCC 首长度前缀 = 首 NAL 3 载荷",
            3L,
            ByteBuffer.wrap(mdat.body, 0, 4).int.toLong() and 0xFFFFFFFFL,
        )
    }

    // ── 分段参数(daemon RenditionConfig 同值锚)──────────────────────────

    @Test
    fun `分段参数与 daemon RenditionConfig 同值锚`() {
        assertEquals(4000L, TranscodeParams.SEGMENT_MS)
        assertEquals(1080, TranscodeParams.TARGET_HEIGHT)
        assertEquals(4_000_000, TranscodeParams.BITRATE_MIN_BPS)
        assertEquals(6_000_000, TranscodeParams.BITRATE_MAX_BPS)
        assertTrue(
            "实编码率须在窗内",
            TranscodeParams.TARGET_BITRATE_BPS in TranscodeParams.BITRATE_MIN_BPS..TranscodeParams.BITRATE_MAX_BPS,
        )
        assertTrue(
            "切段下限 < 段长(段长稳定窗)",
            TranscodeParams.SEGMENT_CUT_MIN_MS < TranscodeParams.SEGMENT_MS,
        )
    }

    @Test
    fun `1080p 封顶缩放 低清不放大 偶数对齐`() {
        assertEquals(Pair(1920, 1080), targetDims(3840, 2160))
        assertEquals(Pair(1920, 1080), targetDims(1920, 1080))
        assertEquals(Pair(854, 480), targetDims(854, 480))
        assertEquals(Pair(2, 2), targetDims(1, 1))
    }

    // ── 段界规则与 ASC 种子(T13 评审 Critical #1 / Important #2)─────────

    @Test
    fun `段界规则 同步样先收当前段再开新段 前段无尾同步 后段首样同步`() {
        // 纯函数钉面(cutBeforeSync):空段首样恒入(段 1 首 IDR 开段);
        // 非空段见同步且累计 ≥ 下限 = 切;非同步样/累计不足恒不切
        assertTrue(cutBeforeSync(hasSamples = true, accumulatedMs = 4_000, isSync = true, minMs = 2_000))
        assertFalse(cutBeforeSync(hasSamples = false, accumulatedMs = 0, isSync = true, minMs = 2_000))
        assertFalse(cutBeforeSync(hasSamples = true, accumulatedMs = 4_000, isSync = false, minMs = 2_000))
        assertFalse(cutBeforeSync(hasSamples = true, accumulatedMs = 1_000, isSync = true, minMs = 2_000))

        // SegmentMuxer 级重放管线用法:段 1 = P、P(尾样非同步)→ 同步样判
        // 切先收段 → 段 2 首样 = sync。trun 样表逐样 flags 断言(段独立面)。
        val m = muxer()
        m.beginFragment(1)
        m.addVideoSample(0, keyFrame = false, annexBSample())
        m.addVideoSample(1_000_000, keyFrame = false, annexBSample())
        // 累计 = 2000ms(pts 跨 1s + 末样时长外推 1s)= 恰达下限
        assertEquals(2_000L, m.videoAccumulatedMs())
        assertTrue(
            "第三样(同步)到达应触发切段判据",
            cutBeforeSync(m.fragmentHasSamples, m.videoAccumulatedMs(), true, TranscodeParams.SEGMENT_CUT_MIN_MS),
        )
        val seg1 = m.finishFragment()
        m.beginFragment(2)
        m.addVideoSample(2_000_000, keyFrame = true, annexBSample())
        val seg2 = m.finishFragment()
        assertEquals("段 1 = 两 P 帧(无尾同步样)", listOf(0x01010000L, 0x01010000L), videoTrunFlags(seg1))
        assertEquals("段 2 首样 = sync(关键帧开段)", listOf(0x02000000L), videoTrunFlags(seg2))
    }

    /** moof → 首 traf(video)→ trun 样表 flags(逐样;trun 载荷 =
     *  version/flags 4 + count 4 + data_offset 4 + 逐样 dur/size/flags 12)。 */
    private fun videoTrunFlags(seg: ByteArray): List<Long> {
        val moof = boxes(seg).single { it.type == "moof" }
        val traf = boxes(moof.body).first { it.type == "traf" }
        val trun = box("trun", traf.body)
        val count = trun.u32(4).toInt()
        return (0 until count).map { trun.u32(12 + it * 12 + 8) }
    }

    @Test
    fun `音频 ASC 种子 直通臂取源 csd 转编臂恒空待编码器 ASC`() {
        // 转编臂源 csd-0 非 ASC 形(如 AC3 的 dac3 盒)——误种会错填 esds 且
        // 令 muxer「音频 csd 齐」等待条件假满足(评审 Important #2)
        val asc = byteArrayOf(0x12, 0x10)
        assertEquals(asc.toList(), audioAscSeedFor(passthrough = true, sourceCsd = asc)!!.toList())
        assertEquals(null, audioAscSeedFor(passthrough = false, sourceCsd = asc))
        assertEquals(null, audioAscSeedFor(passthrough = false, sourceCsd = null))
    }

    // ── Annex-B → AVCC 转换 ───────────────────────────────────────────────

    @Test
    fun `AnnexB 拆NAL toAvcC 记录与 toAvccSample 长度前缀`() {
        val csd = byteArrayOf(
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28,
            0, 0, 0, 1, 0x68, 0x11, 0x22,
        )
        val record = AnnexB.toAvcC(csd)
        assertEquals(
            byteArrayOf(
                1, 0x64, 0x00, 0x28, 0xFF.toByte(), 0xE1.toByte(),
                0, 4, 0x67, 0x64, 0x00, 0x28,
                1, 0, 3, 0x68, 0x11, 0x22,
            ).toList(),
            record.toList(),
        )
        // 样面:3 字节起始码形亦认(00 00 01)
        val sample = byteArrayOf(0, 0, 1, 0x41, 0x02, 0, 0, 1, 0x41, 0x03)
        assertEquals(
            byteArrayOf(0, 0, 0, 2, 0x41, 0x02, 0, 0, 0, 2, 0x41, 0x03).toList(),
            AnnexB.toAvccSample(sample).toList(),
        )
    }
}
