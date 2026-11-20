//! Stamps reth's version metadata into a binary from its build script.
//!
//! Version metadata changes with every commit, and the build timestamp with every build. A
//! library that stamped it would rebuild with every crate that depends on it, so binaries stamp
//! it in their own build script and read it with `reth_node_core::reth_version_metadata!`.
//!
//! ```ignore
//! // build.rs
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     reth_node_version_build::emit_version_metadata()
//! }
//! ```
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

use smolgen::{Build, Cargo, Emitter, Gitcl};
use std::{env, error::Error};

/// Emits the version metadata of the binary being built as `cargo:rustc-env` instructions.
///
/// Call it from the binary's build script. The package version, features and profile are the
/// binary's own.
pub fn emit_version_metadata() -> Result<(), Box<dyn Error>> {
    let mut emitter = Emitter::default();

    emitter.add_instructions(&Build::builder().build_timestamp(true).build())?;
    emitter.add_instructions(&Cargo::builder().features(true).target_triple(true).build())?;

    // Tracking the git HEAD reruns the build script on every commit and checkout. Debug builds
    // skip it and keep the SHA from the script's last run. Release profiles keep tracking HEAD, so
    // their version always matches the checkout.
    let git_builder = Gitcl::builder()
        .describe(false, true, None)
        .dirty(true)
        .sha(false)
        .rerun_on_head_change(env::var("PROFILE")? != "debug")
        .build();
    emitter.add_instructions(&git_builder)?;

    emitter.emit_and_set()?;
    let sha = env::var("VERGEN_GIT_SHA")?;
    let sha_short = &sha[0..7];

    let is_dirty = env::var("VERGEN_GIT_DIRTY")? == "true";
    // > git describe --always --tags
    // if not on a tag: v0.2.0-beta.3-82-g1939939b
    // if on a tag: v0.2.0-beta.3
    // git picks the abbreviated hash length from the repository size, so match any SHA prefix.
    let not_on_tag = env::var("VERGEN_GIT_DESCRIBE")?
        .rsplit_once("-g")
        .is_some_and(|(_, hash)| sha.starts_with(hash));
    let version_suffix = if is_dirty || not_on_tag { "-dev" } else { "" };
    println!("cargo:rustc-env=RETH_VERSION_SUFFIX={version_suffix}");

    // Set short SHA
    println!("cargo:rustc-env=VERGEN_GIT_SHA_SHORT={}", &sha[..8]);

    // Set the build profile
    let out_dir = env::var("OUT_DIR")?;
    let profile = out_dir.rsplit(std::path::MAIN_SEPARATOR).nth(3).ok_or("unexpected OUT_DIR")?;
    println!("cargo:rustc-env=RETH_BUILD_PROFILE={profile}");

    // The binary's version from its Cargo.toml.
    let pkg_version = env::var("CARGO_PKG_VERSION")?;

    // The short version information for reth.
    // - The latest version from Cargo.toml
    // - The short SHA of the latest commit.
    // Example: 0.1.0 (defa64b2)
    println!("cargo:rustc-env=RETH_SHORT_VERSION={pkg_version}{version_suffix} ({sha_short})");

    // LONG_VERSION
    // The long version information for reth, one line per variable since a variable can't hold a
    // newline.
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
    println!("cargo:rustc-env=RETH_LONG_VERSION_0=Version: {pkg_version}{version_suffix}");
    println!("cargo:rustc-env=RETH_LONG_VERSION_1=Commit SHA: {sha}");
    println!(
        "cargo:rustc-env=RETH_LONG_VERSION_2=Build Timestamp: {}",
        env::var("VERGEN_BUILD_TIMESTAMP")?
    );
    println!(
        "cargo:rustc-env=RETH_LONG_VERSION_3=Build Features: {}",
        env::var("VERGEN_CARGO_FEATURES")?
    );
    println!("cargo:rustc-env=RETH_LONG_VERSION_4=Build Profile: {profile}");

    // The version information for reth formatted for P2P (devp2p).
    // - The latest version from Cargo.toml
    // - The target triple
    //
    // Example: reth/v0.1.0-alpha.1-428a6dc2f/aarch64-apple-darwin
    println!(
        "cargo:rustc-env=RETH_P2P_CLIENT_VERSION={}",
        format_args!("reth/v{pkg_version}-{sha_short}/{}", env::var("VERGEN_CARGO_TARGET_TRIPLE")?)
    );

    Ok(())
}
