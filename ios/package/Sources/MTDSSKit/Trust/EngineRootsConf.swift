/// EngineRootsConf — `{home}/engine-roots.conf` 读写(T10 Ruling L;与
/// crates/ios-bridge `parse_engine_roots` **同口径跨语言锚定**:一行一路径,
/// `#` 注释行与空行忽略——Rust 侧 lines()+trim+skip 消费,本侧只写):
/// - App 为唯一写入方:账本 ``TrustedRootsStore/resolveAll()`` 的存活根 +
///   收件箱(recv_dir)经 ``merge(inbox:trusted:)`` 合并后落盘;
/// - 引擎 `mtdss_ios_start` 读它并与 Documents 并集注入授权集;增删信任
///   目录 = 本文件重写 + 引擎重启(Ruling L,重启 ~1-2s 低频可接受)。

import Foundation

public enum EngineRootsConf {

    /// conf 文件名(桥内同名派生;两侧不得漂移)。
    public static let fileName = "engine-roots.conf"

    /// 序列化:一行一路径 + 尾换行(空集 = 空文件内容)。
    public static func serialize(paths: [String]) -> String {
        paths.map { "\($0)\n" }.joined()
    }

    /// 原子落盘至 `{home}/engine-roots.conf`(home 不存在由调用方保证;
    /// 引擎 start 前必经此面,boot 装配先建目录)。
    public static func write(home: String, paths: [String]) throws {
        let url = URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent(fileName)
        try Data(serialize(paths: paths).utf8).write(to: url, options: .atomic)
    }

    /// 引擎授权根合并(纯函数):收件箱恒首位(recv_dir 未就绪不编造),
    /// 信任根保序去重(与收件箱重复的路径同样只留一份)。
    public static func merge(inbox: String?, trusted: [String]) -> [String] {
        var out: [String] = []
        if let inbox, !inbox.isEmpty {
            out.append(inbox)
        }
        for path in trusted where !out.contains(path) {
            out.append(path)
        }
        return out
    }
}
