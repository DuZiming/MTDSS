package com.mtdss.android.fdpass

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asObject
import com.mtdss.android.ipc.toCanonicalJson
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * fdpass 线格式编解码 JVM 单测:帧字节与 crates/ipc/src/fdpass.rs 的线协议
 * 逐字节锁定(pinned 样本;payload 键序 = serde_json BTreeMap 字母序,与
 * daemon json! 宏序列化同口径)。P4a:0x02 dest_fd/0x03 dest_commit 已随
 * 收件 fd 桥退役(值不复用),在册 kind = 0x01/0x04/0x05/0x81。
 */
class FdWireTest {

    /** 帧字节断言助手:帧头(kind + BE len)+ 期望 payload JSON 字符串(UTF-8)。 */
    private fun assertFrame(kind: Int, payloadJson: String, actual: ByteArray) {
        val payload = payloadJson.toByteArray(Charsets.UTF_8)
        val expected = ByteArray(FdWire.HEADER_LEN + payload.size)
        expected[0] = kind.toByte()
        expected[1] = (payload.size ushr 24).toByte()
        expected[2] = (payload.size ushr 16).toByte()
        expected[3] = (payload.size ushr 8).toByte()
        expected[4] = payload.size.toByte()
        payload.copyInto(expected, FdWire.HEADER_LEN)
        assertArrayEquals(expected, actual)
    }

    @Test
    fun `0x05 发布应答 ok 形态与 pinned 字节一致`() {
        // {"ok":true,"request_id":42} → 27 字节 payload;帧 = 05 0000001b 7b 22…
        assertEquals(0x05, FdWire.KIND_PUBLISH_RESULT)
        val frame = FdWire.encodeFrame(
            FdWire.KIND_PUBLISH_RESULT,
            FdWire.publishResultPayload(42, ok = true),
        )
        assertFrame(0x05, """{"ok":true,"request_id":42}""", frame)
        // 全帧 hex 钉死(ok:true 形态无 error 键;与 fdpass.rs write_frame 布局一一对应)
        assertEquals(
            "050000001b7b226f6b223a747275652c22726571756573745f6964223a34327d",
            frame.joinToString("") { "%02x".format(it) },
        )
    }

    @Test
    fun `0x05 发布失败形态 error 字母序与 Rust json 宏一致`() {
        // daemon FdBridgeHandler 0x05 落点读 ok(必填)+ error(可选);失败形态
        // json!({"request_id": id, "ok": false, "error": reason}) → serde_json
        // 字母序:error, ok, request_id;CJK 原样直出(不转义)
        val payload = FdWire.publishResultPayload(7, ok = false, error = "发布拷贝失败: 磁盘满")
        assertEquals(
            """{"error":"发布拷贝失败: 磁盘满","ok":false,"request_id":7}""",
            payload.toCanonicalJson(),
        )
        assertFrame(
            0x05,
            """{"error":"发布拷贝失败: 磁盘满","ok":false,"request_id":7}""",
            FdWire.encodeFrame(FdWire.KIND_PUBLISH_RESULT, payload),
        )
    }

    @Test
    fun `0x81 ack payload 解析(键序无关 error 可缺省)`() {
        // ack ok 形态(daemon 侧字母序写出;解析与键序无关,乱序样本同样过)
        val ack = FdWire.parseAck(
            JVal.parse("""{"request_id":42,"ok":true,"error":null}""").asObject(),
        )
        assertEquals(FdAck(42, true, null), ack)

        val ackErr = FdWire.parseAck(
            JVal.parse("""{"error":"目的地请求已结束(超时或已交付)","ok":false,"request_id":9}""").asObject(),
        )
        assertEquals(FdAck(9, false, "目的地请求已结束(超时或已交付)"), ackErr)

        // error 键缺省(协议允许 error? 可选)
        val ackNoError = FdWire.parseAck(JVal.parse("""{"ok":true,"request_id":3}""").asObject())
        assertEquals(FdAck(3, true, null), ackNoError)
    }

    @Test
    fun `帧头解析 BE 长度与 64KiB 上限`() {
        // 0x05 | len=0x00000100(256):BE 解析成功样本(勿用超限长度)
        val (kind, len) = FdWire.parseHeader(byteArrayOf(0x05, 0x00, 0x00, 0x01, 0x00))
        assertEquals(0x05, kind)
        assertEquals(0x00000100, len)

        // 恰上限:65536(0x00010000)合法
        FdWire.parseHeader(byteArrayOf(0x81.toByte(), 0x00, 0x01, 0x00, 0x00))

        // 超上限:65537 → 拒(镜像 read_frame 的超限断连判定)
        assertThrows(FdWireException::class.java) {
            FdWire.parseHeader(byteArrayOf(0x81.toByte(), 0x00, 0x01, 0x00, 0x01))
        }
    }

    @Test
    fun `编码超限 payload 即拒`() {
        val big = JVal.Obj("x" to JVal.str("a".repeat(FdWire.MAX_PAYLOAD_BYTES)))
        assertThrows(FdWireException::class.java) {
            FdWire.encodeFrame(FdWire.KIND_PUBLISH_RESULT, big)
        }
    }

    @Test
    fun `解码器逐字节喂入跨帧切分`() {
        val f1 = FdWire.encodeFrame(
            FdWire.KIND_PUBLISH_RESULT,
            FdWire.publishResultPayload(1, ok = true),
        )
        val f2 = FdWire.encodeFrame(
            FdWire.KIND_ACK,
            JVal.Obj("ok" to JVal.bool(true), "request_id" to JVal.num(2)),
        )
        val stream = f1 + f2

        val decoder = FdWireDecoder()
        assertTrue(decoder.atFrameBoundary())
        val out = mutableListOf<DecodedFrame>()
        // 逐字节喂:帧恰在末字节处完成
        stream.forEach { b -> out += decoder.feed(byteArrayOf(b)) }
        assertEquals(2, out.size)
        assertEquals(FdWire.KIND_PUBLISH_RESULT, out[0].kind)
        assertArrayEquals("""{"ok":true,"request_id":1}""".toByteArray(), out[0].payload)
        assertEquals(FdWire.KIND_ACK, out[1].kind)
        assertArrayEquals("""{"ok":true,"request_id":2}""".toByteArray(), out[1].payload)
        assertTrue(decoder.atFrameBoundary())

        // 大块喂入一次到位(等价切分)
        val out2 = FdWireDecoder().feed(stream)
        assertEquals(2, out2.size)
        assertEquals(out[0], out2[0])
        assertEquals(out[1], out2[1])

        // 尾部截断:末帧差 3 字节未完成 → 只产出首帧,末帧不产出
        assertEquals(1, FdWireDecoder().feed(stream.copyOf(stream.size - 3)).size)
    }

    @Test
    fun `0x04 在场查询帧 payload 字母序与 pinned 字节一致`() {
        // daemon json!({"request_id": id, "exists": b, "size": n}) → 字母序:
        // exists, request_id, size;CJK 原样直出(F7 dest_query 应答面)
        val present = FdWire.destQueryPayload(requestId = 42, exists = true, size = 1024)
        assertEquals(
            """{"exists":true,"request_id":42,"size":1024}""",
            present.toCanonicalJson(),
        )
        assertFrame(
            0x04,
            """{"exists":true,"request_id":42,"size":1024}""",
            FdWire.encodeFrame(FdWire.KIND_DEST_QUERY, present),
        )
        assertEquals(0x04, FdWire.KIND_DEST_QUERY)
        // 不在场形态:size 恒 0(行缺席/尺寸不符都折算 false)
        val absent = FdWire.destQueryPayload(requestId = 7, exists = false, size = 0)
        assertEquals("""{"exists":false,"request_id":7,"size":0}""", absent.toCanonicalJson())
    }

    @Test
    fun `0x0D seek_fd 交付形态与 pinned 字节一致`() {
        // V4-1 T8 可 seek fd 源桥:fd 随帧交付,payload 仅 {request_id}
        // (镜像 0x01 source_fd 的交付形;账本 0x0A 与 serve_delete 撞号,
        // 改 0x0D——0x0B/0x0C 已为缩略图/转码通道预留,见 FdWire 表注)
        assertEquals(0x0D, FdWire.KIND_SEEK_FD)
        val frame = FdWire.encodeFrame(FdWire.KIND_SEEK_FD, FdWire.seekFdPayload(42))
        assertFrame(0x0D, """{"request_id":42}""", frame)
        assertEquals(
            "0d000000117b22726571756573745f6964223a34327d",
            frame.joinToString("") { "%02x".format(it) },
        )
    }

    @Test
    fun `0x0D 拒开形态 error 字母序与 Rust json 宏一致`() {
        // 无 fd 的 0x0D = App 拒开/不可开(payload 可带 error 文案;daemon
        // SeekBridge 落点读 error 词面)→ {error, ok:false, request_id} 字母序
        val payload = FdWire.refusePayload(7, "文档不可开(授权被撤)")
        assertEquals(
            """{"error":"文档不可开(授权被撤)","ok":false,"request_id":7}""",
            payload.toCanonicalJson(),
        )
        assertFrame(
            0x0D,
            """{"error":"文档不可开(授权被撤)","ok":false,"request_id":7}""",
            FdWire.encodeFrame(FdWire.KIND_SEEK_FD, payload),
        )
    }

    @Test
    fun `0x0B thumb_data 交付形态与 pinned 字节一致`() {
        // V4-1 T9 缩略图桥:JPEG 落 App 缩略图临时目录,{ok, path, request_id}
        // 字母序、无 fd(64 KiB payload 装不下千像素 JPEG——字节走临时件,
        // serve_export/photo_export 同轨;daemon ThumbBridge 落点读 path)
        assertEquals(0x0B, FdWire.KIND_THUMB_DATA)
        val frame = FdWire.encodeFrame(
            FdWire.KIND_THUMB_DATA,
            FdWire.exportReplyPayload(42, "/data/user/0/com.mtdss/files/mtdss/thumb-serve/42-x.jpg"),
        )
        assertFrame(
            0x0B,
            """{"ok":true,"path":"/data/user/0/com.mtdss/files/mtdss/thumb-serve/42-x.jpg","request_id":42}""",
            frame,
        )
    }

    @Test
    fun `0x0B thumb_data 拒答形态 error 字母序与 Rust json 宏一致`() {
        // ok:false 的 0x0B = App 不可解/拒(payload error 词面透传;daemon
        // ThumbBridge 落点收 Err → 引擎面落既有回退,缩略图缺位只是降级)
        val payload = FdWire.refusePayload(7, "无法解码(格式不支持)")
        assertEquals(
            """{"error":"无法解码(格式不支持)","ok":false,"request_id":7}""",
            payload.toCanonicalJson(),
        )
        assertFrame(
            0x0B,
            """{"error":"无法解码(格式不支持)","ok":false,"request_id":7}""",
            FdWire.encodeFrame(FdWire.KIND_THUMB_DATA, payload),
        )
    }
}
