package com.mtdss.android.inbox

import java.io.FileDescriptor

/**
 * 收件箱暂存面(T6 fs.request_dest 协议的磁盘侧抽象;协议编排见 [InboxLogic],
 * MediaStore 实现见 MediaStoreInboxStore)。JVM 单测注入伪实现——
 * MediaStore 行为不可 JVM 验证,本接口把「可验的决策逻辑」与「须实机验的
 * 磁盘面」切开。
 *
 * rel 语义(与 daemon collect_items/intake 同源):顶层文件 = 文件名;
 * 顶层目录 = `目录名/内部相对路径`(`/` 分隔)。
 */
interface InboxStore {

    /**
     * 全新收暂存:新建收件箱条目(实机 = MediaStore IS_PENDING=1 插入
     * Downloads/MTDSS;同名 pending 残留先行清除,见实现注释)。失败抛
     * [InboxStoreException] → [InboxLogic] 转 0x02 拒开。
     */
    fun create(rel: String, size: Long): InboxHandle

    /**
     * 重开既有暂存(**不截断**;T6 续传语义:断点续传 = 平台层重开 fd +
     * 对账续写)。无在途暂存返回 null → 拒开(NOT_FOUND)——daemon 侧
     * 随之清陈旧记录、重试轮按全新重收。
     */
    fun reopen(rel: String, size: Long): InboxHandle?

    /**
     * 终件在场查询(F7 fs.query_dest 的磁盘侧):rel 对应落位**行在且
     * SIZE 与 [size] 吻合** → true;无行/size 不符/路径非法/查询失败 → false——
     * false 使 daemon 对账不 Skip、引擎走 FULL(失败方向铁律:宁可重传,
     * 不可假完成;实机 = MediaStore DISPLAY_NAME+RELATIVE_PATH 行查询)。
     */
    fun exists(rel: String, size: Long): Boolean
}

/**
 * 一份已打开的收件箱暂存(handle 由 [InboxStore] 产出;生命周期由
 * [InboxLogic] 驱动:reveal/discard/release 恰一次)。
 */
interface InboxHandle {

    /** 暂存 fd(0x02 随帧交付;SCM_RIGHTS 在 sendmsg 时复制,本副本随后即可关)。 */
    val rawFd: FileDescriptor

    /** 关闭本地 fd 副本(暂存条目保留;应答完成但 0x03 未至的路径)。 */
    fun release()

    /** 落定:揭开暂存条目(实机 = MediaStore IS_PENDING=0,系统文件管理器可见)。 */
    fun reveal()

    /** 清除暂存条目(0x03 done:false 失败收尾 / 应答被拒 / 发送失败路径)。 */
    fun discard()
}

/** 暂存打开失败(MediaStore 插入/打开失败等;message 会进 0x02 拒开文案)。 */
class InboxStoreException(message: String, cause: Throwable? = null) : Exception(message, cause)
