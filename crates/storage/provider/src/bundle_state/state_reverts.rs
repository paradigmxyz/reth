use crate::DatabaseProviderRW;
use rayon::slice::ParallelSliceMut;
use reth_db::{tables, Database};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO, DbDupCursorRW},
    models::{AccountBeforeTx, BlockNumberAddress},
    transaction::DbTxMut,
};
use reth_primitives::{BlockNumber, StorageEntry, B256, U256};
use reth_storage_errors::db::DatabaseError;
use revm::db::states::{PlainStateReverts, PlainStorageRevert, RevertToSlot};
use std::iter::Peekable;

/// Revert of the state.
#[derive(Debug, Default)]
pub struct StateReverts(pub PlainStateReverts);

impl From<PlainStateReverts> for StateReverts {
    fn from(revm: PlainStateReverts) -> Self {
        Self(revm)
    }
}

impl StateReverts {
    /// Write reverts to database.
    ///
    /// `Note::` Reverts will delete all wiped storage from plain state.
    pub fn write_to_db<DB>(
        self,
        provider: &DatabaseProviderRW<DB>,
        first_block: BlockNumber,
    ) -> Result<(), DatabaseError>
    where
        DB: Database,
    {
        // Write storage changes
        tracing::trace!(target: "provider::reverts", "Writing storage changes");
        let mut storages_cursor =
            provider.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        let mut storage_changeset_cursor =
            provider.tx_ref().cursor_dup_write::<tables::StorageChangeSets>()?;
        for (block_index, mut storage_changes) in self.0.storage.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;

            tracing::trace!(target: "provider::reverts", block_number, "Writing block change");
            // sort changes by address.
            storage_changes.par_sort_unstable_by_key(|a| a.address);
            for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
                let storage_id = BlockNumberAddress((block_number, address));

                let mut storage = storage_revert
                    .into_iter()
                    .map(|(k, v)| (B256::new(k.to_be_bytes()), v))
                    .collect::<Vec<_>>();
                // sort storage slots by key.
                storage.par_sort_unstable_by_key(|a| a.0);

                // If we are writing the primary storage wipe transition, the pre-existing plain
                // storage state has to be taken from the database and written to storage history.
                // See [StorageWipe::Primary] for more details.
                let mut wiped_storage = Vec::new();
                if wiped {
                    tracing::trace!(target: "provider::reverts", ?address, "Wiping storage");
                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        wiped_storage.push((entry.key, entry.value));
                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            wiped_storage.push((entry.key, entry.value))
                        }
                    }
                }

                tracing::trace!(target: "provider::reverts", ?address, ?storage, "Writing storage reverts");
                for (key, value) in StorageRevertsIter::new(storage, wiped_storage) {
                    storage_changeset_cursor.append_dup(storage_id, StorageEntry { key, value })?;
                }
            }
        }

        // Write account changes
        tracing::trace!(target: "provider::reverts", "Writing account changes");
        let mut account_changeset_cursor =
            provider.tx_ref().cursor_dup_write::<tables::AccountChangeSets>()?;

        for (block_index, mut account_block_reverts) in self.0.accounts.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;
            // Sort accounts by address.
            account_block_reverts.par_sort_by_key(|a| a.0);

            for (address, info) in account_block_reverts {
                account_changeset_cursor.append_dup(
                    block_number,
                    AccountBeforeTx { address, info: info.map(Into::into) },
                )?;
            }
        }

        Ok(())
    }
}

/// Iterator over storage reverts.
/// See [`StorageRevertsIter::next`] for more details.
#[allow(missing_debug_implementations)]
pub struct StorageRevertsIter<R: Iterator, W: Iterator> {
    reverts: Peekable<R>,
    wiped: Peekable<W>,
}

impl<R, W> StorageRevertsIter<R, W>
where
    R: Iterator<Item = (B256, RevertToSlot)>,
    W: Iterator<Item = (B256, U256)>,
{
    /// Create a new iterator over storage reverts.
    pub fn new(
        reverts: impl IntoIterator<IntoIter = R>,
        wiped: impl IntoIterator<IntoIter = W>,
    ) -> Self {
        Self { reverts: reverts.into_iter().peekable(), wiped: wiped.into_iter().peekable() }
    }

    /// Consume next revert and return it.
    fn next_revert(&mut self) -> Option<(B256, U256)> {
        self.reverts.next().map(|(key, revert)| (key, revert.to_previous_value()))
    }

    /// Consume next wiped storage and return it.
    fn next_wiped(&mut self) -> Option<(B256, U256)> {
        self.wiped.next()
    }
}

impl<R, W> Iterator for StorageRevertsIter<R, W>
where
    R: Iterator<Item = (B256, RevertToSlot)>,
    W: Iterator<Item = (B256, U256)>,
{
    type Item = (B256, U256);

    /// Iterate over storage reverts and wiped entries and return items in the sorted order.
    /// NOTE: The implementation assumes that inner iterators are already sorted.
    fn next(&mut self) -> Option<Self::Item> {
        match (self.reverts.peek(), self.wiped.peek()) {
            (Some(revert), Some(wiped)) => {
                // Compare the keys and return the lesser.
                use std::cmp::Ordering;
                match revert.0.cmp(&wiped.0) {
                    Ordering::Less => self.next_revert(),
                    Ordering::Greater => self.next_wiped(),
                    Ordering::Equal => {
                        // Keys are the same, decide which one to return.
                        let (key, revert_to) = *revert;

                        let value = match revert_to {
                            // If the slot is some, prefer the revert value.
                            RevertToSlot::Some(value) => value,
                            // If the slot was destroyed, prefer the database value.
                            RevertToSlot::Destroyed => wiped.1,
                        };

                        // Consume both values from inner iterators.
                        self.next_revert();
                        self.next_wiped();

                        Some((key, value))
                    }
                }
            }
            (Some(_revert), None) => self.next_revert(),
            (None, Some(_wiped)) => self.next_wiped(),
            (None, None) => None,
        }
    }
}
