use std::error::Error;
use vergen::EmitBuilder;

/// Emits instructions with a Git SHA enabled.
fn main() -> Result<(), Box<dyn Error>> {
    // Emit the instructions
    EmitBuilder::builder().git_sha(true).emit()?;
    Ok(())
}
