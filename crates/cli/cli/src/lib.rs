//! Cli abstraction for reth based nodes.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::borrow::Cow;

/// Reth based node cli.
///
/// This trait is supposed to be implemented by the main struct of the CLI.
///
/// It provides commonly used functionality for running commands and information about the CL, such
/// as the name and version.
pub trait RethCli: Sized {
    /// The name of the implementation, eg. `reth`, `op-reth`, etc.
    fn name(&self) -> Cow<'static, str>;

    /// The version of the node, such as `reth/v1.0.0`
    fn version(&self) -> Cow<'static, str>;
}
