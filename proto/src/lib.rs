pub mod ws {
    include!(concat!(env!("OUT_DIR"), "/mtdss.ws.rs"));
}
pub mod signal {
    include!(concat!(env!("OUT_DIR"), "/mtdss.signal.rs"));
}
pub mod manifest {
    include!(concat!(env!("OUT_DIR"), "/mtdss.manifest.rs"));
}
pub mod ctrl {
    include!(concat!(env!("OUT_DIR"), "/mtdss.ctrl.rs"));
}

pub const MANIFEST_VERSION: u32 = 2;
/// ctrl 协议版本:v2 = v1(list/stat)基础上两拨增量——第一拨(Task 4)
/// Delete(请求 4/响应 6)、Move(5/7)、Copy(8/9);第二拨(Task 7)
/// Orchestrate/CancelJob/JobQuery(请求与响应同占 10/11/12)。版本不符
/// 双向仍回 UNSUPPORTED_VERSION。
pub const CTRL_VERSION: u32 = 2;
