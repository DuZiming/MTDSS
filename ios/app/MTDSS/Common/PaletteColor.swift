/// FavoritesPalette hex → SwiftUI Color(V3-3 T17 自 FavoritesMixedView 私有
/// 工具抽出共用:首页根列表色点串/管理屏色点/混合页色点三面同源)。Kit 侧
/// 色板只存无 # 前缀 6 位 hex 字符串(T14:值过 IPC 面),Color 拼装属渲染面
/// (app 目标),检视在案。

import SwiftUI

extension Color {
    /// 6 位无 # 前缀 hex(如 "158cd5",spec §13.4 首值)→ Color(非法串按
    /// 0 值黑;来源恒为 ``FavoritesPalette.hexes`` 白名单,非法输入不入渲染
    /// 路径)。
    init(paletteHex hex: String) {
        var value: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&value)
        self.init(
            red: Double((value >> 16) & 0xFF) / 255,
            green: Double((value >> 8) & 0xFF) / 255,
            blue: Double(value & 0xFF) / 255,
        )
    }
}
