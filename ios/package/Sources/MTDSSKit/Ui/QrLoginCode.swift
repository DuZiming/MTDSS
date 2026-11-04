/// QrLoginCode — 扫码登录二维码内容的纯解析面(V3-1;无 IO,Kit 可测):
/// 二维码内容 = daemon 拼 `https://www.mtdss.top/qr?t=<token>`(login.qr_begin
/// 契约;spec §3.2,服务端不参与)。扫描器(App 侧 AVFoundation)扫得任意
/// 字符串交 ``parse``:命中本家 URL 形态 → 取 t 参数为 qr token;其余(网页
/// 链接/文本/他家二维码)→ nil,调用方给「非 MTDSS 登录码」文案。
/// token 交 ``QrApprovalModel`` 走 claim/approve 授权闭环(Ruling R8 透传
/// op 已通,弹层接线在 App 侧 ProfileView)。

import Foundation

public enum QrLoginCode {

    /// 二维码 URL 固定前缀(daemon account.rs qr_url 同字面)。
    static let prefix = "https://www.mtdss.top/qr?t="

    /// 从扫描原文提取 qr token:严格匹配前缀(大小写敏感,防钓鱼变体),
    /// token = 其后首个查询参数值(URL 编码不解——daemon 生成的 token 为
    /// URL 安全字母表,原样直用)。空白首尾容忍;空 token → nil。
    public static func parse(_ raw: String) -> String? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix(prefix) else { return nil }
        let rest = text.dropFirst(prefix.count)
        // t 为首个查询参数;若后面还拼了别的参数(&)只取本段
        let token = rest.split(separator: "&", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? ""
        guard !token.isEmpty else { return nil }
        return token
    }
}
