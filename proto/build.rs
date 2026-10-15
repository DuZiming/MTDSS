fn main() -> Result<(), Box<dyn std::error::Error>> {
    // No .proto sources until Task 2; it adds the prost_build::compile_protos call.
    println!("cargo:rerun-if-changed=proto/signal.proto");
    println!("cargo:rerun-if-changed=proto/manifest.proto");
    Ok(())
}
