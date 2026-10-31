import Foundation
import MTDSSKit

/// IPC 契约 golden 目录定位与读取(crates/ipc/contract = 单一真源,daemon 契约
/// 测试/Kotlin 客户端同源)。Ruling F 取「直读仓库目录」路线:不经拷贝、不经
/// 符号链接、不经 Package 资源——测试源码经 `#filePath`(编译期锚定
/// Tests/MTDSSKitTests/)上溯 4 级得仓库根,再拼 `crates/ipc/contract`。
/// golden 与客户端永远同仓共迁,漂移无门(无需一致性校验测试)。
enum Goldens {

    static let dir: URL = {
        var url = URL(fileURL: #filePath) // …/ios/package/Tests/MTDSSKitTests/Goldens.swift
        for _ in 0..<4 { url.deleteLastPathComponent() } // → 仓库根
        let dir = url.appendingPathComponent("crates/ipc/contract")
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: dir.path, isDirectory: &isDir), isDir.boolValue else {
            fatalError("找不到 golden 目录 crates/ipc/contract(候选:\(dir.path))")
        }
        return dir
    }()

    /// 读 golden 文件原始字节(含尾换行 = 线上帧原文)。
    static func read(_ name: String) -> Data {
        guard let data = FileManager.default.contents(atPath: dir.appendingPathComponent(name).path) else {
            fatalError("golden 文件缺失: \(name)")
        }
        return data
    }

    static func text(_ name: String) -> String {
        String(decoding: read(name), as: UTF8.self)
    }

    /// 目录下全部文件名(排序后)。
    static func allNames() -> [String] {
        (try? FileManager.default.contentsOfDirectory(atPath: dir.path))?.sorted()
            ?? fatalError("golden 目录不可枚举: \(dir.path)")
    }

    /// golden 应答/事件行重写 id(其余内容原样——golden 本就是 serde_json 的
    /// canonical 键序,经 canonical 重排逐字节不变)。伪 server 应答客户端
    /// 任意 id 的调用时用。
    static func withId(_ bytes: Data, _ id: UInt64) -> Data {
        guard let parsed = try? JSONParser.parse(String(decoding: bytes, as: UTF8.self)),
              case .object(var obj) = parsed else {
            fatalError("golden 帧非 JSON 对象")
        }
        obj["id"] = .uint(id)
        return Data((JSONValue.object(obj).canonical + "\n").utf8)
    }
}
