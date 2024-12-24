#![allow(missing_docs)]

use std::{env, error::Error};
use vergen::EmitBuilder;

fn main() -> Result<(), Box<dyn Error>> {
    // Emit the instructions
    EmitBuilder::builder()
        .git_describe(false, true, None)
        .git_dirty(true)
        .git_sha(false)
        .build_timestamp()
        .cargo_features()
        .cargo_target_triple()
        .emit_and_set()?;

    let sha = env::var("VERGEN_GIT_SHA")?;
    let sha_short = &sha[0..7];

    let is_dirty = env::var("VERGEN_GIT_DIRTY")? == "true";
    // > git describe --always --tags
    // if not on a tag: v0.2.0-beta.3-82-g1939939b
    // if on a tag: v0.2.0-beta.3
    let not_on_tag = env::var("VERGEN_GIT_DESCRIBE")?.ends_with(&format!("-g{sha_short}"));
    let version_suffix = if is_dirty || not_on_tag { "-dev" } else { "" };
    println!("cargo:rustc-env=RETH_VERSION_SUFFIX={}", version_suffix);

    // Set short SHA
    println!("cargo:rustc-env=VERGEN_GIT_SHA_SHORT={}", &sha[..8]);

    // Set the build profile
    let out_dir = env::var("OUT_DIR")?;
    let profile = if out_dir.contains('/') {
        // Unix-style paths
        let parts: Vec<_> = out_dir.split('/').collect();
        if parts.len() >= 4 {
            parts[parts.len() - 4]
        } else {
            // Try Windows-style paths as fallback
            let parts: Vec<_> = out_dir.split('\\').collect();
            parts[parts.len() - 4]
        }
    } else {
        // Windows-style paths
        let parts: Vec<_> = out_dir.split('\\').collect();
        parts[parts.len() - 4]
    };
    println!("cargo:rustc-env=RETH_BUILD_PROFILE={profile}");

    // Set formatted version strings
    let pkg_version = env!("CARGO_PKG_VERSION");

    // The short version information for reth.
    // - The latest version from Cargo.toml
    // - The short SHA of the latest commit.
    // Example: 0.1.0 (defa64b2)
    println!(
        "cargo:rustc-env=RETH_SHORT_VERSION={}",
        format!("{pkg_version}{version_suffix} ({sha_short})")
    );

    // LONG_VERSION
    // The long version information for reth.
    //
    // - The latest version from Cargo.toml + version suffix (if any)
    // - The full SHA of the latest commit
    // - The build datetime
    // - The build features
    // - The build profile
    //
    // Example:
    //
    // ```text
    // Version: 0.1.0
    // Commit SHA: defa64b2
    // Build Timestamp: 2023-05-19T01:47:19.815651705Z
    // Build Features: jemalloc
    // Build Profile: maxperf
    // ```
    println!(
        "cargo:rustc-env=RETH_LONG_VERSION={}",
        format!(
            "Version: {pkg_version}{version_suffix}\n\
             Commit SHA: {sha}\n\
             Build Timestamp: {}\n\
             Build Features: {}\n\
             Build Profile: {profile}",
            env::var("VERGEN_BUILD_TIMESTAMP")?,
            env::var("VERGEN_CARGO_FEATURES")?,
        )
    );

    panic!("debug");

    Ok(())
}
