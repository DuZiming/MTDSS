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
pub const CTRL_VERSION: u32 = 1;
