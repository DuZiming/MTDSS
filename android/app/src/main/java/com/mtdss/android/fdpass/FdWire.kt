package com.mtdss.android.fdpass

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.req
import java.io.ByteArrayOutputStream

/**
 * fdpass 线格式纯编解码(镜像 crates/ipc/src/fdpass.rs 的线协议注释,
 * 双端实现必须逐字节一致):
 *
 * ```text
 * kind u8 | payload_len u32(大端) | payload(UTF-8 JSON,≤ 64 KiB)
 * ```
 *
 * fd 不进线格式,经 SCM_RIGHTS 控制消息随帧附带(0 或 1 个)——编解码层
 * 只见字节,fd 的收发在 [FdSocketConnection] 传输面。
 *
 * kind 常量(与 fdpass.rs 同表):
 *
 * | 值   | 名          | 方向        | payload                          | fd |
 * |------|-------------|-------------|----------------------------------|----|
 * | 0x01 | source_fd   | App→daemon  | `{request_id}`                   | 1  |
 * | 0x02 | dest_fd     | App→daemon  | `{request_id}`(拒开:`+ok:false,error?`)| 1/0 |
 * | 0x03 | dest_commit | daemon→App  | `{request_id, done}`             | 0  |
 * | 0x81 | ack         | daemon→App  | `{request_id, ok, error?}`       | 0  |
 *
 * payload 的 JSON 键序:daemon 侧 serde_json 默认 BTreeMap = 字母序,
 * [JVal.toCanonicalJson] 同口径——[encodeFrame] 出的帧与 Rust `json!` 宏
 * 序列化逐字节一致(FdWireTest 钉死样本字节)。
 */
object FdWire {

    /** 0x01 source_fd:App→daemon,上传源 fd(T10 消费;payload `{request_id}` + 1 fd)。 */
    const val KIND_SOURCE_FD: Int = 0x01

    /** 0x02 dest_fd:App→daemon,下载目标暂存 fd(payload `{request_id}` + 1 fd;无 fd = 拒开)。 */
    const val KIND_DEST_FD: Int = 0x02

    /** 0x03 dest_commit:daemon→App,落盘确认(payload `{request_id, done}`)。 */
    const val KIND_DEST_COMMIT: Int = 0x03

    /** 0x81 ack:daemon→App 应答(payload `{request_id, ok, error?}`)。 */
    const val KIND_ACK: Int = 0x81

    /** 帧头长度:`kind u8 | len u32 BE`。 */
    const val HEADER_LEN: Int = 5

    /** 单帧 payload(JSON)字节上限:64 KiB(镜像 fdpass.rs MAX_PAYLOAD_BYTES)。 */
    const val MAX_PAYLOAD_BYTES: Int = 64 * 1024

    /** 整帧编码:帧头(kind + payload 长度大端)+ payload。payload 超 64 KiB 即拒(镜像 write 前置校验)。 */
    fun encodeFrame(kind: Int, payload: JVal): ByteArray {
        val body = payload.toCanonicalJson().toByteArray(Charsets.UTF_8)
        if (body.size > MAX_PAYLOAD_BYTES) {
            throw FdWireException("fdpass payload ${body.size} 字节,超 $MAX_PAYLOAD_BYTES 上限")
        }
        val frame = ByteArray(HEADER_LEN + body.size)
        frame[0] = kind.toByte()
        frame[1] = (body.size ushr 24).toByte()
        frame[2] = (body.size ushr 16).toByte()
        frame[3] = (body.size ushr 8).toByte()
        frame[4] = body.size.toByte()
        body.copyInto(frame, HEADER_LEN)
        return frame
    }

    /** 帧头解析 → (kind, payload 长度);长度超上限抛 [FdWireException](调用方断连)。 */
    fun parseHeader(header: ByteArray): Pair<Int, Int> {
        require(header.size == HEADER_LEN) { "帧头须为 $HEADER_LEN 字节" }
        val kind = header[0].toInt() and 0xFF
        val len = ((header[1].toInt() and 0xFF) shl 24) or
            ((header[2].toInt() and 0xFF) shl 16) or
            ((header[3].toInt() and 0xFF) shl 8) or
            (header[4].toInt() and 0xFF)
        if (len > MAX_PAYLOAD_BYTES) {
            throw FdWireException("fdpass 帧 payload $len 字节,超 $MAX_PAYLOAD_BYTES 上限")
        }
        return kind to len
    }

    /** 0x02 交付形态 payload:`{request_id}`。 */
    fun destFdPayload(requestId: Long): JVal.Obj = JVal.Obj("request_id" to JVal.num(requestId))

    /** 0x02 拒开形态 payload:`{request_id, ok:false, error}`(镜像 e2e 伪 provider 的 json! 形态)。 */
    fun destFdRefusePayload(requestId: Long, error: String): JVal.Obj = JVal.Obj(
        "request_id" to JVal.num(requestId),
        "ok" to JVal.bool(false),
        "error" to JVal.str(error),
    )

    /** 0x81 ack payload 解析:`{request_id, ok, error?}`(error 可缺省;解析与键序无关)。 */
    fun parseAck(payload: JVal.Obj): FdAck = FdAck(
        requestId = payload.req("request_id").asLong(),
        ok = payload.req("ok").asBoolean(),
        error = payload.opt("error").asStringOrNull(),
    )

    /** 0x03 dest_commit payload 解析:`{request_id, done}`。 */
    fun parseCommit(payload: JVal.Obj): FdCommit = FdCommit(
        requestId = payload.req("request_id").asLong(),
        done = payload.req("done").asBoolean(),
    )
}

/** 0x81 ack 应答(requestId 单调全局,daemon 侧不复用)。 */
data class FdAck(val requestId: Long, val ok: Boolean, val error: String?)

/** 0x03 dest_commit:done=true 揭开暂存(IS_PENDING=0);false 清暂存。 */
data class FdCommit(val requestId: Long, val done: Boolean)

/** fdpass 线协议异常(帧超限/截断/非法 JSON/fd 数异常等;调用方按断连收口)。 */
class FdWireException(message: String) : Exception(message)

/**
 * 字节流式帧解码器(无 IO 状态;测试伪 server 复用)。feed 任意分块,
 * 帧完成即产出;帧头长度超上限抛 [FdWireException](镜像 read_frame 口径)。
 */
class FdWireDecoder {

    /** 半成品缓冲(帧头或 payload 的未完成部分)。 */
    private val partial = ByteArrayOutputStream(64)

    /** 当前期待的阶段:待读帧头(5)→ 待读 payload(len)。 */
    private var expect: Int = FdWire.HEADER_LEN

    /** 当前帧的 kind(帧头读完后有效)。 */
    private var kind: Int = -1

    fun atFrameBoundary(): Boolean = partial.size() == 0

    /** 喂入一段字节;返回本次完成的帧(kind + payload 原始字节,可能空表)。 */
    fun feed(bytes: ByteArray, off: Int = 0, len: Int = bytes.size - off): List<DecodedFrame> {
        var pos = off
        val end = off + len
        val out = mutableListOf<DecodedFrame>()
        while (pos < end) {
            val take = minOf(expect, end - pos)
            partial.write(bytes, pos, take)
            pos += take
            expect -= take
            if (expect > 0) break // 本段字节耗尽,帧未完成
            if (kind < 0) {
                // 帧头齐:切阶段(payload 长度经 FdWire 上限校验)
                val (k, plen) = FdWire.parseHeader(partial.toByteArray())
                kind = k
                expect = plen
                partial.reset()
                if (plen == 0) {
                    out += DecodedFrame(kind, ByteArray(0))
                    kind = -1
                    expect = FdWire.HEADER_LEN
                }
            } else {
                // payload 齐:一帧完成
                out += DecodedFrame(kind, partial.toByteArray())
                partial.reset()
                kind = -1
                expect = FdWire.HEADER_LEN
            }
        }
        return out
    }
}

/** 解码完成的一帧(payload 为原始字节,由调用方按需解析 JSON)。 */
class DecodedFrame(val kind: Int, val payload: ByteArray) {
    override fun equals(other: Any?): Boolean =
        other is DecodedFrame && other.kind == kind && other.payload.contentEquals(payload)

    override fun hashCode(): Int = kind * 31 + payload.contentHashCode()
}
