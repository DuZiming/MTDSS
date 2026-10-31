// swift-tools-version: 5.9
import PackageDescription

/// MTDSS iOS 客户端核心包:ipc JSON-RPC 客户端(NDJSON over UDS)+ 契约模型
/// + 数据门面(golden 双端锁定,与 android/.../ipc、crates/ipc/contract 同源,
/// 协议事实对照 crates/ipc/src/lib.rs)。
let package = Package(
    name: "MTDSSKit",
    platforms: [
        .iOS(.v26),
        .macOS(.v13),
    ],
    products: [
        .library(name: "MTDSSKit", targets: ["MTDSSKit"]),
    ],
    targets: [
        .target(name: "MTDSSKit"),
        .testTarget(name: "MTDSSKitTests", dependencies: ["MTDSSKit"]),
    ],
)
