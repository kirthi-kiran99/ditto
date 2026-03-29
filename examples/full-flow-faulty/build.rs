fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reuse the proto definition from full-flow — same service contract,
    // faulty implementation lives only in the Rust handler logic.
    tonic_build::compile_protos("../full-flow/proto/fulfillment.proto")?;
    Ok(())
}
