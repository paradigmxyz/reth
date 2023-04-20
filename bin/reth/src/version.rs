//! This module contains the version message for the program.
include!(concat!(env!("OUT_DIR"), "/built.rs"));

const VERSION: &str = PKG_VERSION;
const NAME: &str = PKG_NAME;
const SHA: Option<&str> = GIT_COMMIT_HASH_SHORT;
const OS: &str = std::env::consts::OS;

/// The version message for the current program, like
/// `reth/v0.1.0/macos-6fc95a5/`
pub fn version_message() -> String {
    format!("{}/v{}/{}-{}", NAME, VERSION, OS, SHA.unwrap())
}
