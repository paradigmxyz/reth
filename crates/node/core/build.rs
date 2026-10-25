#![allow(missing_docs)]

use smolgen::{Build, Cargo, Emitter, Gitcl};
use std::{collections::BTreeMap, env, error::Error};

fn main() -> Result<(), Box<dyn Error>> {
    let mut emitter = Emitter::default();

    let build_builder = Build::builder().build_timestamp(true).build();

    emitter.add_instructions(&build_builder)?;

    let cargo_builder = Cargo::builder().features(true).target_triple(true).build();

    emitter.add_instructions(&cargo_builder)?;

    let git_builder = Gitcl::builder().describe(false, true, None).dirty(true).sha(false).build();

    emitter.add_instructions(&git_builder)?;

    let mut instructions = Vec::new();
    emitter.emit_to(&mut instructions)?;
    // Tracking the git HEAD reruns this script, and rebuilds everything above this crate, on every
    // commit and checkout. Debug builds skip it and keep the SHA from the last run of this script.
    // Release profiles keep tracking HEAD, so their version always matches the checkout.
    let skip_head_tracking = env::var("PROFILE")? == "debug";
    for line in String::from_utf8(instructions)?.lines() {
        let is_git_path = line.starts_with("cargo:rerun-if-changed=") &&
            line != "cargo:rerun-if-changed=build.rs";
        if !(skip_head_tracking && is_git_path) {
            println!("{line}");
        }
    }

    let vars = emitter
        .cargo_rustc_env_map()
        .iter()
        .map(|(key, value)| (key.name(), value.as_str()))
        .collect::<BTreeMap<_, _>>();
    let var = |name: &str| vars.get(name).copied().ok_or_else(|| format!("{name} is not set"));

    let sha = var("VERGEN_GIT_SHA")?;
    let sha_short = &sha[0..7];

    let is_dirty = var("VERGEN_GIT_DIRTY")? == "true";
    // > git describe --always --tags
    // if not on a tag: v0.2.0-beta.3-82-g1939939b
    // if on a tag: v0.2.0-beta.3
    // git picks the abbreviated hash length from the repository size, so match any SHA prefix.
    let not_on_tag = var("VERGEN_GIT_DESCRIBE")?
        .rsplit_once("-g")
        .is_some_and(|(_, hash)| sha.starts_with(hash));
    let version_suffix = if is_dirty || not_on_tag { "-dev" } else { "" };
    println!("cargo:rustc-env=RETH_VERSION_SUFFIX={version_suffix}");

    // Set short SHA
    println!("cargo:rustc-env=VERGEN_GIT_SHA_SHORT={}", &sha[..8]);

    // Set the build profile
    let out_dir = env::var("OUT_DIR").unwrap();
    let profile = out_dir.rsplit(std::path::MAIN_SEPARATOR).nth(3).unwrap();
    println!("cargo:rustc-env=RETH_BUILD_PROFILE={profile}");

    // Set formatted version strings
    let pkg_version = env!("CARGO_PKG_VERSION");

    // The short version information for reth.
    // - The latest version from Cargo.toml
    // - The short SHA of the latest commit.
    // Example: 0.1.0 (defa64b2)
    println!("cargo:rustc-env=RETH_SHORT_VERSION={pkg_version}{version_suffix} ({sha_short})");

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
    println!("cargo:rustc-env=RETH_LONG_VERSION_0=Version: {pkg_version}{version_suffix}");
    println!("cargo:rustc-env=RETH_LONG_VERSION_1=Commit SHA: {sha}");
    println!(
        "cargo:rustc-env=RETH_LONG_VERSION_2=Build Timestamp: {}",
        var("VERGEN_BUILD_TIMESTAMP")?
    );
    println!(
        "cargo:rustc-env=RETH_LONG_VERSION_3=Build Features: {}",
        var("VERGEN_CARGO_FEATURES")?
    );
    println!("cargo:rustc-env=RETH_LONG_VERSION_4=Build Profile: {profile}");

    // The version information for reth formatted for P2P (devp2p).
    // - The latest version from Cargo.toml
    // - The target triple
    //
    // Example: reth/v0.1.0-alpha.1-428a6dc2f/aarch64-apple-darwin
    println!(
        "cargo:rustc-env=RETH_P2P_CLIENT_VERSION={}",
        format_args!("reth/v{pkg_version}-{sha_short}/{}", var("VERGEN_CARGO_TARGET_TRIPLE")?)
    );

    Ok(())
}
