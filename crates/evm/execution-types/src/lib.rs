//! Commonly used types for (EVM) block execution.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

mod chain;
pub use chain::*;

mod execute;
pub use execute::*;

mod execution_outcome;
pub use execution_outcome::*;

/// Bincode-compatible serde implementations for commonly used types for (EVM) block execution.
///
/// `bincode` crate doesn't work with optionally serializable serde fields, but some of the
/// execution types require optional serialization for RPC compatibility. This module makes so that
/// all fields are serialized.
///
/// Read more: <https://github.com/bincode-org/bincode/issues/326>
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    pub use super::chain::serde_bincode_compat::*;
}
