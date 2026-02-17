//! An integration of `reth-trie` with `reth-db`.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod changesets;
pub use changesets::*;
mod hashed_cursor;
mod prefix_set;
mod proof;
mod state;
mod storage;
mod trie_cursor;
mod witness;

pub use hashed_cursor::{
    DatabaseHashedAccountCursor, DatabaseHashedCursorFactory, DatabaseHashedStorageCursor,
};
pub use prefix_set::load_prefix_sets_with_provider;
pub use proof::{DatabaseProof, DatabaseStorageProof};
pub use state::{from_reverts_auto, DatabaseHashedPostState, DatabaseStateRoot};
pub use storage::{hashed_storage_from_reverts_with_provider, DatabaseStorageRoot};
pub use trie_cursor::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
    LegacyKeyAdapter, PackedAccountsTrie, PackedKeyAdapter, PackedStoragesTrie,
    StorageTrieEntryLike, TrieKeyAdapter, TrieTableAdapter,
};
pub use witness::DatabaseTrieWitness;

/// Dispatches a trie operation using the correct [`TrieKeyAdapter`] based on storage settings.
///
/// The first argument must implement
/// [`StorageSettingsCache`](reth_storage_api::StorageSettingsCache). Inside the closure body, `$A`
/// is a type alias for either [`PackedKeyAdapter`] or [`LegacyKeyAdapter`].
///
/// # Example
///
/// ```ignore
/// reth_trie_db::with_adapter!(provider, |A| {
///     let factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
///     // ...
/// })
/// ```
#[macro_export]
macro_rules! with_adapter {
    ($settings_provider:expr, |$A:ident| $body:expr) => {
        if $settings_provider.cached_storage_settings().is_v2() {
            {
                type $A = $crate::PackedKeyAdapter;
                $body
            }
        } else {
            {
                type $A = $crate::LegacyKeyAdapter;
                $body
            }
        }
    };
}
