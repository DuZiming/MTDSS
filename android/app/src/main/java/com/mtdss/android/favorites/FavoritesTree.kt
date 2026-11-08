package com.mtdss.android.favorites

import com.mtdss.android.ipc.DirectoryDevice

/**
 * 快捷树「成员行」纯投影层(V3-4 T9;spec §4.1 成员行树 + §4.10「devices
 * 快照 join 设备名(未及退化 id)」;iOS 首页 FavoritesView 的 memberRow/
 * deviceName/isMemberOnline 三私函的 Kotlin 同构,T10 快捷目录 tab 消费):
 *
 * - 行标签 = **「设备名 - 目录名」**(§16.3 词面):设备名 = devices 快照
 *   join(同 id 取首个),未及(快照缺该设备)退化 id;目录名 = 路径末段
 *   显示名(双分隔符 `/` `\` 裁切——DirEntry.path 由 daemon 按目标平台原生
 *   分隔符下发,严禁以 '/' 单独切分;iOS BrowseFormat.displayName 同象:
 *   剥空段取末段,无段回退整路径)。
 * - **离线判定 = devices 快照 join**:快照缺该设备 = 离线(快照未及窗恒
 *   保守离线,刷新/重进归位);在场但 online=false 亦离线(iOS
 *   `contains { $0.deviceId == m.device && $0.online }` 同象)。离线整行
 *   灰显禁点禁展开归 T10 渲染。
 * - 行序 = 成员添加序(不重排不合并;同设备多成员各行并存)。
 *
 * 本模块只做投影,不发请求;纯函数一律返回新值,不改入参。
 */
object FavoritesTree {

    /** 成员行:展示标签 + 成员身份(device/path)+ 离线旗(灰显判定源)。 */
    data class MemberRow(
        /** 「设备名 - 目录名」(§16.3 词面;a11y 全形同串,iOS 成员行同构)。 */
        val displayName: String,
        val deviceId: String,
        val path: String,
        val offline: Boolean,
    )

    /** 单快捷的成员行投影(行序 = 成员添加序)。 */
    fun members(favorite: Favorite, devices: List<DirectoryDevice>): List<MemberRow> =
        favorite.members.map { m ->
            MemberRow(
                displayName = deviceName(devices, m.device) + " - " + dirName(m.path),
                deviceId = m.device,
                path = m.path,
                offline = !isOnline(devices, m.device),
            )
        }

    /** 设备显示名(快照 join,同 id 取首个;未及退化 id——词面前半)。 */
    fun deviceName(devices: List<DirectoryDevice>, deviceId: String): String =
        devices.firstOrNull { it.deviceId == deviceId }?.name ?: deviceId

    /** 成员在线判定(devices 快照 join;快照查无此设备 = 离线,online=false 同)。 */
    fun isOnline(devices: List<DirectoryDevice>, deviceId: String): Boolean =
        devices.any { it.deviceId == deviceId && it.online }

    /** 路径末段显示名(iOS BrowseFormat.displayName 同象):按 `/` `\` 双
     * 分隔符切分、剥空段取末段;无段(根/空串)回退整路径。 */
    fun dirName(path: String): String =
        path.split('/', '\\').lastOrNull { it.isNotEmpty() } ?: path
}
