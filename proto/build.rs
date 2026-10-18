fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(
        &[
            "proto/signal.proto",
            "proto/e2e.proto",
            "proto/manifest.proto",
            "proto/ctrl.proto",
        ],
        &["proto"],
    )?;
    println!("cargo:rerun-if-changed=proto/signal.proto");
    println!("cargo:rerun-if-changed=proto/e2e.proto");
    println!("cargo:rerun-if-changed=proto/manifest.proto");
    println!("cargo:rerun-if-changed=proto/ctrl.proto");
    Ok(())
}
