//! This module contains helpful utilities related to populating changesets tables.

mod state_reverts;
pub use state_reverts::StorageRevertsIter;

use alloy_primitives::{Address, B256};
use reth_db_api::models::{AccountBeforeTx, StorageBeforeTx};
use revm_database::states::PlainStorageRevert;

/// Converts a single block's account reverts from a
/// [`PlainStateReverts`](revm_database::states::PlainStateReverts) into [`AccountBeforeTx`]
/// entries, matching the format stored in the database.
pub fn reverts_to_account_changesets(
    reverts: impl IntoIterator<Item = (Address, Option<impl Into<reth_primitives_traits::Account>>)>,
) -> Vec<AccountBeforeTx> {
    reverts
        .into_iter()
        .map(|(address, info)| AccountBeforeTx { address, info: info.map(Into::into) })
        .collect()
}

/// Converts a single block's storage reverts from a
/// [`PlainStateReverts`](revm_database::states::PlainStateReverts) into [`StorageBeforeTx`]
/// entries, matching the format stored in the database.
pub fn reverts_to_storage_changesets(
    reverts: impl IntoIterator<Item = PlainStorageRevert>,
) -> Vec<StorageBeforeTx> {
    reverts
        .into_iter()
        .flat_map(|revert| {
            revert.storage_revert.into_iter().map(move |(key, value)| StorageBeforeTx {
                address: revert.address,
                key: B256::from(key.to_be_bytes()),
                value: value.to_previous_value(),
            })
        })
        .collect()
}
