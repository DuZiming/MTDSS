package com.mtdss.android.send

/**
 * fd 桥上传条目数护栏:daemon 侧 `MAX_BRIDGE_ITEMS`(crates/daemon/src/jobs.rs)
 * 的前置镜像。超限 daemon 在 transfer.start 入口即拒(同词面),此处先行
 * 预检省一次 RPC 且文案直达 UI;daemon 错误(词面相同)照常透传兜底。
 */
object BridgeLimits {

    /** 上限(条目数 = 峰值 fd 句柄数;Android 进程 fd 软限 1024 扣除余量取 512)。 */
    const val MAX_BRIDGE_ITEMS: Int = 512

    /** 条目数预检:null = 通过;非 null = 错误文案(与 daemon 词面逐字一致)。 */
    fun itemCountError(count: Int): String? =
        if (count > MAX_BRIDGE_ITEMS) "桥接条目过多($count 项,上限 $MAX_BRIDGE_ITEMS):请分批发送" else null
}
