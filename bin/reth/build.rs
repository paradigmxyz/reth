#![allow(missing_docs)]

use smolgen::{Build, Emitter, Gitcl};
use std::{env, error::Error};

// The build timestamp is stamped here rather than in `reth-node-core`: it changes on every run of
// this script, and in a library it would rebuild every crate that depends on it.
fn main() -> Result<(), Box<dyn Error>> {
    // Rerun on HEAD changes where `reth-node-core` does, so the timestamp never predates the SHA
    // shown beside it.
    let git_builder =
        Gitcl::builder().sha(true).rerun_on_head_change(env::var("PROFILE")? != "debug").build();

    Emitter::default()
        .add_instructions(&Build::builder().build_timestamp(true).build())?
        .add_instructions(&git_builder)?
        .emit()?;
    Ok(())
}
