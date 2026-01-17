//! Chain and trie data types for reth.
//!
//! This crate contains the [`Chain`] type representing a chain of blocks and their final state,
//! as well as [`DeferredTrieData`] for handling asynchronously computed trie data and
//! re-exports [`LazyTrieData`] for lazy initialization.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod chain;
pub use chain::*;

mod deferred_trie;
pub use deferred_trie::*;

// Re-export LazyTrieData from trie-common for convenience
pub use reth_trie_common::LazyTrieData;

/// Bincode-compatible serde implementations for chain types.
///
/// `bincode` crate doesn't work with optionally serializable serde fields, but some of the
/// chain types require optional serialization for RPC compatibility. This module makes so that
/// all fields are serialized.
///
/// Read more: <https://github.com/bincode-org/bincode/issues/326>
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::chain::serde_bincode_compat::*;
}
