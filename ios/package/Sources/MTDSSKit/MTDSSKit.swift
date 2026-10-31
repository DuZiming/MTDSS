/// MTDSSKit — MTDSS iOS 客户端核心包(v2-p5 Task 3)。
///
/// 模块组成(实现落地中,TDD 先红后绿):
/// - `Ipc/`:NDJSON JSON-RPC 客户端(线格式/契约模型/actor 客户端/POSIX UDS 端点);
/// - `Data/`:MtdssRepository(async 门面)+ EventHub(事件扇出)。
///
/// 协议单一真源:crates/ipc/src/lib.rs + crates/ipc/contract/*.json(golden)。
