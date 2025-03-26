//! This crate defines a set of commonly used cli utils.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod allocator;

/// Helper function to load a secret key from a file.
pub mod load_secret_key;
pub use load_secret_key::get_secret_key;

/// Cli parsers functions.
pub mod parsers;
pub use parsers::{
    hash_or_num_value_parser, parse_duration_from_secs, parse_duration_from_secs_or_ms,
    parse_socket_address,
};

#[cfg(all(unix, any(target_env = "gnu", target_os = "macos")))]
pub mod sigsegv_handler;

/// Signal handler to extract a backtrace from stack overflow.
///
/// This is a no-op because this platform doesn't support our signal handler's requirements.
#[cfg(not(all(unix, any(target_env = "gnu", target_os = "macos"))))]
pub mod sigsegv_handler {
    /// No-op function.
    pub fn install() {}
}
