// swift-tools-version: 6.2
import PackageDescription

/// MTDSS iOS 客户端核心包:ipc JSON-RPC 客户端(NDJSON over UDS)+ 契约模型
/// + 数据门面(golden 双端锁定,与 android/.../ipc、crates/ipc/contract 同源,
/// 协议事实对照 crates/ipc/src/lib.rs)。
///
/// 注:iOS 26 平台声明需 PackageDescription 6.2;语言面保持 Swift 5 模式
/// (swiftLanguageModes: [.v5],Xcode 26 工具链,Swift 5.9+ 语法)。
/// macOS 下限 14(T10:@Observable/Observation 框架的最低版本;macOS 形态
/// 仅为宿主单测存在,开发盒 macOS 26.3)。
let package = Package(
    name: "MTDSSKit",
    platforms: [
        .iOS(.v26),
        .macOS(.v14),
    ],
    products: [
        .library(name: "MTDSSKit", targets: ["MTDSSKit"]),
    ],
    targets: [
        .target(name: "MTDSSKit"),
        .testTarget(name: "MTDSSKitTests", dependencies: ["MTDSSKit"]),
    ],
    swiftLanguageModes: [.v5],
)
