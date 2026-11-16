#![allow(missing_docs)]

use smolgen::{Build, Emitter};
use std::error::Error;

// The build timestamp is stamped here rather than in `reth-node-core`: it changes on every run of
// this script, and in a library it would rebuild every crate that depends on it.
fn main() -> Result<(), Box<dyn Error>> {
    Emitter::default().add_instructions(&Build::builder().build_timestamp(true).build())?.emit()?;
    Ok(())
}
