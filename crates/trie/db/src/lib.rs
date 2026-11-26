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
mod verify;

pub use hashed_cursor::{
    DatabaseHashedAccountCursor, DatabaseHashedCursorFactory, DatabaseHashedStorageCursor,
};
pub use prefix_set::load_prefix_sets_with_provider;
pub use proof::{DatabaseProof, DatabaseStorageProof};
pub use reth_db_api::{
    models::TrieKeyEncoding,
    tables::{MemcmpAccountsTrie, MemcmpStoragesTrie, PackedAccountsTrie, PackedStoragesTrie},
};
pub use state::{DatabaseHashedPostState, DatabaseStateRoot};
pub use storage::{hashed_storage_from_reverts_with_provider, DatabaseStorageRoot};
pub use trie_cursor::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
    LegacyKeyAdapter, MemcmpKeyAdapter, PackedKeyAdapter, StorageTrieEntryLike, TrieKeyAdapter,
    TrieTableAdapter,
};
pub use verify::DatabaseVerifier;

/// Dispatches a trie operation using the correct [`TrieKeyAdapter`] based on storage settings.
///
/// The first argument must implement
/// [`StorageSettingsCache`](reth_storage_api::StorageSettingsCache). Inside the closure body, `$A`
/// is a type alias for [`LegacyKeyAdapter`], [`PackedKeyAdapter`] or [`MemcmpKeyAdapter`], as
/// selected by the settings' [`TrieKeyEncoding`].
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
        match $settings_provider.cached_storage_settings().trie_key_encoding() {
            $crate::TrieKeyEncoding::Legacy => {
                type $A = $crate::LegacyKeyAdapter;
                $body
            }
            $crate::TrieKeyEncoding::Packed => {
                type $A = $crate::PackedKeyAdapter;
                $body
            }
            $crate::TrieKeyEncoding::Memcmp => {
                type $A = $crate::MemcmpKeyAdapter;
                $body
            }
        }
    };
}
