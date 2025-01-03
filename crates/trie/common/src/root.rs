//! Common root computation functions.

// Re-export for convenience.
#[doc(inline)]
pub use alloy_trie::root::{
    state_root, state_root_ref_unhashed, state_root_unhashed, state_root_unsorted, storage_root,
    storage_root_unhashed, storage_root_unsorted,
};
