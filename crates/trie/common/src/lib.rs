//! Commonly used types for trie usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// The implementation of hash builder.
pub mod hash_builder;

mod account;
pub use account::TrieAccount;

mod mask;
pub(crate) use mask::StoredTrieMask;

mod nibbles;
pub use nibbles::{Nibbles, StoredNibbles, StoredNibblesSubKey};

pub mod nodes;
pub use nodes::StoredBranchNode;

mod storage;
pub use storage::StorageTrieEntry;

mod subnode;
pub use subnode::StoredSubNode;

mod proofs;
#[cfg(any(test, feature = "test-utils"))]
pub use proofs::triehash;
pub use proofs::{AccountProof, StorageProof};

pub mod root;

pub use alloy_trie::{proof, BranchNodeCompact, HashBuilder, TrieMask, EMPTY_ROOT_HASH};
