fn main() -> Result<(), Box<dyn core::error::Error>> {
    tonic_build::compile_protos("proto/exex.proto")?;
    Ok(())
}
