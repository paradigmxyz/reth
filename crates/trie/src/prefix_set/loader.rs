use super::PrefixSetMut;
use derive_more::Deref;
use reth_db::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives::{keccak256, trie::Nibbles, BlockNumber, StorageEntry, B256};
use std::{
    collections::{HashMap, HashSet},
    ops::RangeInclusive,
};

/// Loaded prefix sets.
#[derive(Debug, Default)]
pub struct LoadedPrefixSets {
    /// The account prefix set
    pub account_prefix_set: PrefixSetMut,
    /// The mapping of hashed account key to the corresponding storage prefix set
    pub storage_prefix_sets: HashMap<B256, PrefixSetMut>,
    /// The account keys of destroyed accounts
    pub destroyed_accounts: HashSet<B256>,
}

/// A wrapper around a database transaction that loads prefix sets within a given block range.
#[derive(Debug, Deref)]
pub struct PrefixSetLoader<'a, TX>(&'a TX);

impl<'a, TX> PrefixSetLoader<'a, TX> {
    /// Create a new loader.
    pub fn new(tx: &'a TX) -> Self {
        Self(tx)
    }
}

impl<'a, TX: DbTx> PrefixSetLoader<'a, TX> {
    /// Load all account and storage changes for the given block range.
    pub fn load(
        self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<LoadedPrefixSets, DatabaseError> {
        // Initialize prefix sets.
        let mut loaded_prefix_sets = LoadedPrefixSets::default();

        // Walk account changeset and insert account prefixes.
        let mut account_changeset_cursor = self.cursor_read::<tables::AccountChangeSet>()?;
        let mut account_plain_state_cursor = self.cursor_read::<tables::PlainAccountState>()?;
        for account_entry in account_changeset_cursor.walk_range(range.clone())? {
            let (_, AccountBeforeTx { address, .. }) = account_entry?;
            let hashed_address = keccak256(address);
            loaded_prefix_sets.account_prefix_set.insert(Nibbles::unpack(hashed_address));

            if account_plain_state_cursor.seek_exact(address)?.is_none() {
                loaded_prefix_sets.destroyed_accounts.insert(hashed_address);
            }
        }

        // Walk storage changeset and insert storage prefixes as well as account prefixes if missing
        // from the account prefix set.
        let mut storage_cursor = self.cursor_dup_read::<tables::StorageChangeSet>()?;
        let storage_range = BlockNumberAddress::range(range);
        for storage_entry in storage_cursor.walk_range(storage_range)? {
            let (BlockNumberAddress((_, address)), StorageEntry { key, .. }) = storage_entry?;
            let hashed_address = keccak256(address);
            loaded_prefix_sets.account_prefix_set.insert(Nibbles::unpack(hashed_address));
            loaded_prefix_sets
                .storage_prefix_sets
                .entry(hashed_address)
                .or_default()
                .insert(Nibbles::unpack(keccak256(key)));
        }

        Ok(loaded_prefix_sets)
    }
}
