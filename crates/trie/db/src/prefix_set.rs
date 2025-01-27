use alloy_primitives::{
    map::{HashMap, HashSet},
    BlockNumber, B256,
};
use core::{marker::PhantomData, ops::RangeInclusive};
use derive_more::Deref;
use reth_db::tables;
use reth_db_api::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress},
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives::StorageEntry;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    KeyHasher, Nibbles,
};

/// A wrapper around a database transaction that loads prefix sets within a given block range.
#[derive(Debug)]
pub struct PrefixSetLoader<'a, TX, KH>(&'a TX, PhantomData<KH>);

impl<'a, TX, KH> PrefixSetLoader<'a, TX, KH> {
    /// Create a new loader.
    pub const fn new(tx: &'a TX) -> Self {
        Self(tx, PhantomData)
    }
}

impl<TX, KH> Deref for PrefixSetLoader<'_, TX, KH> {
    type Target = TX;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<TX: DbTx, KH: KeyHasher> PrefixSetLoader<'_, TX, KH> {
    /// Load all account and storage changes for the given block range.
    pub fn load(self, range: RangeInclusive<BlockNumber>) -> Result<TriePrefixSets, DatabaseError> {
        // Initialize prefix sets.
        let mut account_prefix_set = PrefixSetMut::default();
        let mut storage_prefix_sets = HashMap::<B256, PrefixSetMut>::default();
        let mut destroyed_accounts = HashSet::default();

        // Walk account changeset and insert account prefixes.
        let mut account_changeset_cursor = self.cursor_read::<tables::AccountChangeSets>()?;
        let mut account_hashed_state_cursor = self.cursor_read::<tables::HashedAccounts>()?;
        for account_entry in account_changeset_cursor.walk_range(range.clone())? {
            let (_, AccountBeforeTx { address, .. }) = account_entry?;
            let hashed_address = KH::hash_key(address);
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            if account_hashed_state_cursor.seek_exact(hashed_address)?.is_none() {
                destroyed_accounts.insert(hashed_address);
            }
        }

        // Walk storage changeset and insert storage prefixes as well as account prefixes if missing
        // from the account prefix set.
        let mut storage_cursor = self.cursor_dup_read::<tables::StorageChangeSets>()?;
        let storage_range = BlockNumberAddress::range(range);
        for storage_entry in storage_cursor.walk_range(storage_range)? {
            let (BlockNumberAddress((_, address)), StorageEntry { key, .. }) = storage_entry?;
            let hashed_address = KH::hash_key(address);
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            storage_prefix_sets
                .entry(hashed_address)
                .or_default()
                .insert(Nibbles::unpack(KH::hash_key(key)));
        }

        Ok(TriePrefixSets {
            account_prefix_set: account_prefix_set.freeze(),
            storage_prefix_sets: storage_prefix_sets
                .into_iter()
                .map(|(k, v)| (k, v.freeze()))
                .collect(),
            destroyed_accounts,
        })
    }
}
