/// 分享件展示名取名链(R5/J2;纯字符串面,宿主可测):
/// ① suggestedName(非空且非纯扩展名)→ ② 载入件落地名(file representation
/// 落地 URL 的 lastPathComponent)→ ③ 相册源 PHAsset 原名(originalFilename)
/// → ④ 兜底「分享文件.ext」(保留,前置三道后应不可达)。
///
/// 用户实机:相册经 Synshu 分享,条目名变「分享文件/分享文件.jpg」——相册流
/// suggestedName 常空,旧实现只认 suggestedName 一道,落地名与 asset 原名
/// 两道真实名源未用。扩展名不丢不改:①②③ 所得缺扩展时按落地类型补
/// (唯一允许的「改名」);已有扩展(无论异同)原样保——「分享文件.jpg」
/// 形只在 ④ 兜底出现。
///
/// 净化与追加语义与 R10 旧 displayName 逐字同锚(sanitizedName;ext 异名
/// 追加不剥不改),仅取名源扩为四道。
public enum ShareDisplayName {

    /// 取名链求解(fallbackExt = 落地类型首选扩展名,可空;nil/空则 ④ 回
    /// 「分享文件」不带扩展)。
    public static func resolve(
        suggested: String?, loadedName: String?, assetName: String?, fallbackExt: String?,
    ) -> String {
        let ext = (fallbackExt ?? "").lowercased()
        func accept(_ raw: String, pureExtGated: Bool) -> String? {
            let base = ShareQueueStore.sanitizedName(raw)
            if base.isEmpty { return nil }
            // ① 级特规:纯扩展名(如「jpg」「.jpg」)不算名,下沉
            if pureExtGated && isPureExtension(base, ext: fallbackExt) { return nil }
            // 扩展名不丢不改:已有扩展(无论异同)原样保;缺扩展按落地类型补
            let baseExt = (base as NSString).pathExtension.lowercased()
            return (ext.isEmpty || baseExt == ext) ? base : "\(base).\(ext)"
        }
        if let s = suggested, let v = accept(s, pureExtGated: true) { return v }
        if let l = loadedName, let v = accept(l, pureExtGated: false) { return v }
        if let a = assetName, let v = accept(a, pureExtGated: false) { return v }
        return ext.isEmpty ? "分享文件" : "分享文件.\(ext)"
    }

    /// 纯扩展名判定(suggested 面专用):无中缀点且(去前导点后)与落地扩展
    /// 名同字面(如 suggested「jpg」+ jpg);落地扩展名不可知时恒 false
    /// (无从比对,不冤枉真名)。
    static func isPureExtension(_ name: String, ext: String?) -> Bool {
        guard let ext, !ext.isEmpty else { return false }
        var trimmed = name
        if trimmed.hasPrefix(".") { trimmed = String(trimmed.dropFirst()) }
        if trimmed.contains(".") { return false }
        return trimmed.lowercased() == ext.lowercased()
    }
}
