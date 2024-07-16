//! Various branch nodes produced by the hash builder.

mod branch;
pub use branch::{StoredBranchNode, StoredBranchNodeStatic};

pub use alloy_trie::nodes::*;
