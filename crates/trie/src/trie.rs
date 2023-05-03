use crate::{
    account::EthAccount,
    hashed_cursor::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor},
    prefix_set::{PrefixSet, PrefixSetLoader},
    progress::{IntermediateStateRootState, StateRootProgress},
    trie_cursor::{AccountTrieCursor, StorageTrieCursor},
    updates::{TrieKey, TrieOp, TrieUpdates},
    walker::TrieWalker,
    StateRootError, StorageRootError,
};
use reth_db::{tables, transaction::DbTx};
use reth_primitives::{
    keccak256,
    proofs::EMPTY_ROOT,
    trie::{HashBuilder, Nibbles},
    Address, BlockNumber, StorageEntry, H256,
};
use reth_rlp::Encodable;
use std::{collections::HashMap, ops::RangeInclusive};

/// StateRoot is used to compute the root node of a state trie.
pub struct StateRoot<'a, 'b, TX, H> {
    /// A reference to the database transaction.
    pub tx: &'a TX,
    /// The factory for hashed cursors.
    pub hashed_cursor_factory: &'b H,
    /// A set of account prefixes that have changed.
    pub changed_account_prefixes: PrefixSet,
    /// A map containing storage changes with the hashed address as key and a set of storage key
    /// prefixes as the value.
    pub changed_storage_prefixes: HashMap<H256, PrefixSet>,
    /// Previous intermediate state.
    previous_state: Option<IntermediateStateRootState>,
    /// The number of updates after which the intermediate progress should be returned.
    threshold: u64,
}

impl<'a, 'b, TX, H> StateRoot<'a, 'b, TX, H> {
    /// Set the changed account prefixes.
    pub fn with_changed_account_prefixes(mut self, prefixes: PrefixSet) -> Self {
        self.changed_account_prefixes = prefixes;
        self
    }

    /// Set the changed storage prefixes.
    pub fn with_changed_storage_prefixes(mut self, prefixes: HashMap<H256, PrefixSet>) -> Self {
        self.changed_storage_prefixes = prefixes;
        self
    }

    /// Set the threshold.
    pub fn with_threshold(mut self, threshold: u64) -> Self {
        self.threshold = threshold;
        self
    }

    /// Set the threshold to maximum value so that itermediate progress is not returned.
    pub fn with_no_threshold(mut self) -> Self {
        self.threshold = u64::MAX;
        self
    }

    /// Set the previously recorded intermediate state.
    pub fn with_intermediate_state(mut self, state: Option<IntermediateStateRootState>) -> Self {
        self.previous_state = state;
        self
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<'c, HF>(
        self,
        hashed_cursor_factory: &'c HF,
    ) -> StateRoot<'a, 'c, TX, HF> {
        StateRoot {
            tx: self.tx,
            changed_account_prefixes: self.changed_account_prefixes,
            changed_storage_prefixes: self.changed_storage_prefixes,
            threshold: self.threshold,
            previous_state: self.previous_state,
            hashed_cursor_factory,
        }
    }
}

impl<'a, 'tx, TX> StateRoot<'a, 'a, TX, TX>
where
    TX: DbTx<'tx> + HashedCursorFactory<'a>,
{
    /// Create a new [StateRoot] instance.
    pub fn new(tx: &'a TX) -> Self {
        Self {
            tx,
            changed_account_prefixes: PrefixSet::default(),
            changed_storage_prefixes: HashMap::default(),
            previous_state: None,
            threshold: 100_000,
            hashed_cursor_factory: tx,
        }
    }

    /// Given a block number range, identifies all the accounts and storage keys that
    /// have changed.
    ///
    /// # Returns
    ///
    /// An instance of state root calculator with account and storage prefixes loaded.
    pub fn incremental_root_calculator(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, StateRootError> {
        let (account_prefixes, storage_prefixes) = PrefixSetLoader::new(tx).load(range)?;
        Ok(Self::new(tx)
            .with_changed_account_prefixes(account_prefixes)
            .with_changed_storage_prefixes(storage_prefixes))
    }

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes.
    ///
    /// # Returns
    ///
    /// The updated state root.
    pub fn incremental_root(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<H256, StateRootError> {
        tracing::debug!(target: "loader", "incremental state root");
        Self::incremental_root_calculator(tx, range)?.root()
    }

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// Ignores the threshold.
    ///
    /// # Returns
    ///
    /// The updated state root and the trie updates.
    pub fn incremental_root_with_updates(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(H256, TrieUpdates), StateRootError> {
        tracing::debug!(target: "loader", "incremental state root");
        Self::incremental_root_calculator(tx, range)?.root_with_updates()
    }

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    pub fn incremental_root_with_progress(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError> {
        tracing::debug!(target: "loader", "incremental state root with progress");
        Self::incremental_root_calculator(tx, range)?.root_with_progress()
    }
}

impl<'a, 'b, 'tx, TX, H> StateRoot<'a, 'b, TX, H>
where
    TX: DbTx<'tx>,
    H: HashedCursorFactory<'b>,
{
    /// Walks the intermediate nodes of existing state trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder. Collects the updates in the process.
    ///
    /// Ignores the threshold.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation and the trie updates.
    pub fn root_with_updates(self) -> Result<(H256, TrieUpdates), StateRootError> {
        match self.with_no_threshold().calculate(true)? {
            StateRootProgress::Complete(root, updates) => Ok((root, updates)),
            StateRootProgress::Progress(..) => unreachable!(), // unreachable threshold
        }
    }

    /// Walks the intermediate nodes of existing state trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder.
    ///
    /// # Returns
    ///
    /// The state root hash.
    pub fn root(self) -> Result<H256, StateRootError> {
        match self.calculate(false)? {
            StateRootProgress::Complete(root, _) => Ok(root),
            StateRootProgress::Progress(..) => unreachable!(), // update retenion is disabled
        }
    }

    /// Walks the intermediate nodes of existing state trie (if any) and hashed entries. Feeds the
    /// nodes into the hash builder. Collects the updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    pub fn root_with_progress(self) -> Result<StateRootProgress, StateRootError> {
        self.calculate(true)
    }

    fn calculate(self, retain_updates: bool) -> Result<StateRootProgress, StateRootError> {
        tracing::debug!(target: "loader", "calculating state root");
        let mut trie_updates = TrieUpdates::default();

        let mut hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let mut trie_cursor =
            AccountTrieCursor::new(self.tx.cursor_read::<tables::AccountsTrie>()?);

        let (mut walker, mut hash_builder, mut last_account_key, mut last_walker_key) =
            match self.previous_state {
                Some(state) => (
                    TrieWalker::from_stack(
                        &mut trie_cursor,
                        state.walker_stack,
                        self.changed_account_prefixes,
                    ),
                    state.hash_builder,
                    Some(state.last_account_key),
                    Some(state.last_walker_key),
                ),
                None => (
                    TrieWalker::new(&mut trie_cursor, self.changed_account_prefixes),
                    HashBuilder::default(),
                    None,
                    None,
                ),
            };

        walker.set_updates(retain_updates);
        hash_builder.set_updates(retain_updates);

        let mut account_rlp = Vec::with_capacity(128);

        while let Some(key) = last_walker_key.take().or_else(|| walker.key()) {
            // Take the last account key to make sure we take it into consideration only once.
            let (next_key, mut next_account_entry) = match last_account_key.take() {
                // Seek the last processed entry and take the next after.
                Some(account_key) => {
                    hashed_account_cursor.seek(account_key)?;
                    (walker.key(), hashed_account_cursor.next()?)
                }
                None => {
                    if walker.can_skip_current_node {
                        let value = walker.hash().unwrap();
                        let is_in_db_trie = walker.children_are_in_trie();
                        hash_builder.add_branch(key.clone(), value, is_in_db_trie);
                    }

                    let seek_key = match walker.next_unprocessed_key() {
                        Some(key) => key,
                        None => break, // no more keys
                    };

                    (walker.advance()?, hashed_account_cursor.seek(seek_key)?)
                }
            };

            while let Some((hashed_address, account)) = next_account_entry {
                let account_nibbles = Nibbles::unpack(hashed_address);

                if let Some(ref key) = next_key {
                    if key < &account_nibbles {
                        tracing::trace!(target: "loader", "breaking, already detected");
                        break
                    }
                }

                // We assume we can always calculate a storage root without
                // OOMing. This opens us up to a potential DOS vector if
                // a contract had too many storage entries and they were
                // all buffered w/o us returning and committing our intermeditate
                // progress.
                // TODO: We can consider introducing the TrieProgress::Progress/Complete
                // abstraction inside StorageRoot, but let's give it a try as-is for now.
                let storage_root_calculator = StorageRoot::new_hashed(self.tx, hashed_address)
                    .with_hashed_cursor_factory(self.hashed_cursor_factory)
                    .with_changed_prefixes(
                        self.changed_storage_prefixes
                            .get(&hashed_address)
                            .cloned()
                            .unwrap_or_default(),
                    );

                let storage_root = if retain_updates {
                    let (root, updates) = storage_root_calculator.root_with_updates()?;
                    trie_updates.extend(updates.into_iter());
                    root
                } else {
                    storage_root_calculator.root()?
                };

                let account = EthAccount::from(account).with_storage_root(storage_root);

                account_rlp.clear();
                account.encode(&mut &mut account_rlp);

                hash_builder.add_leaf(account_nibbles, &account_rlp);

                // Decide if we need to return intermediate progress.
                let total_updates_len =
                    trie_updates.len() + walker.updates_len() + hash_builder.updates_len();
                if retain_updates && total_updates_len as u64 >= self.threshold {
                    let (walker_stack, walker_updates) = walker.split();
                    let (hash_builder, hash_builder_updates) = hash_builder.split();

                    let state = IntermediateStateRootState {
                        hash_builder,
                        walker_stack,
                        last_walker_key: key,
                        last_account_key: hashed_address,
                    };

                    trie_updates.extend(walker_updates.into_iter());
                    trie_updates.extend_with_account_updates(hash_builder_updates);

                    return Ok(StateRootProgress::Progress(Box::new(state), trie_updates))
                }

                // Move the next account entry
                next_account_entry = hashed_account_cursor.next()?;
            }
        }

        let root = hash_builder.root();

        let (_, walker_updates) = walker.split();
        let (_, hash_builder_updates) = hash_builder.split();

        trie_updates.extend(walker_updates.into_iter());
        trie_updates.extend_with_account_updates(hash_builder_updates);

        Ok(StateRootProgress::Complete(root, trie_updates))
    }
}

/// StorageRoot is used to compute the root node of an account storage trie.
pub struct StorageRoot<'a, 'b, TX, H> {
    /// A reference to the database transaction.
    pub tx: &'a TX,
    /// The factory for hashed cursors.
    pub hashed_cursor_factory: &'b H,
    /// The hashed address of an account.
    pub hashed_address: H256,
    /// The set of storage slot prefixes that have changed.
    pub changed_prefixes: PrefixSet,
}

impl<'a, 'tx, TX> StorageRoot<'a, 'a, TX, TX>
where
    TX: DbTx<'tx> + HashedCursorFactory<'a>,
{
    /// Creates a new storage root calculator given an raw address.
    pub fn new(tx: &'a TX, address: Address) -> Self {
        Self::new_hashed(tx, keccak256(address))
    }

    /// Creates a new storage root calculator given a hashed address.
    pub fn new_hashed(tx: &'a TX, hashed_address: H256) -> Self {
        Self {
            tx,
            hashed_address,
            changed_prefixes: PrefixSet::default(),
            hashed_cursor_factory: tx,
        }
    }
}

impl<'a, 'b, TX, H> StorageRoot<'a, 'b, TX, H> {
    /// Creates a new storage root calculator given an raw address.
    pub fn new_with_factory(tx: &'a TX, hashed_cursor_factory: &'b H, address: Address) -> Self {
        Self::new_hashed_with_factory(tx, hashed_cursor_factory, keccak256(address))
    }

    /// Creates a new storage root calculator given a hashed address.
    pub fn new_hashed_with_factory(
        tx: &'a TX,
        hashed_cursor_factory: &'b H,
        hashed_address: H256,
    ) -> Self {
        Self { tx, hashed_address, changed_prefixes: PrefixSet::default(), hashed_cursor_factory }
    }

    /// Set the changed prefixes.
    pub fn with_changed_prefixes(mut self, prefixes: PrefixSet) -> Self {
        self.changed_prefixes = prefixes;
        self
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<'c, HF>(
        self,
        hashed_cursor_factory: &'c HF,
    ) -> StorageRoot<'a, 'c, TX, HF> {
        StorageRoot {
            tx: self.tx,
            hashed_address: self.hashed_address,
            changed_prefixes: self.changed_prefixes,
            hashed_cursor_factory,
        }
    }
}

impl<'a, 'b, 'tx, TX, H> StorageRoot<'a, 'b, TX, H>
where
    TX: DbTx<'tx>,
    H: HashedCursorFactory<'b>,
{
    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root and storage trie updates for a given address.
    pub fn root_with_updates(&self) -> Result<(H256, TrieUpdates), StorageRootError> {
        self.calculate(true)
    }

    /// Walks the hashed storage table entries for a given address and calculates the storage root.
    ///
    /// # Returns
    ///
    /// The storage root.
    pub fn root(&self) -> Result<H256, StorageRootError> {
        let (root, _) = self.calculate(false)?;
        Ok(root)
    }

    fn calculate(&self, retain_updates: bool) -> Result<(H256, TrieUpdates), StorageRootError> {
        tracing::debug!(target: "trie::storage_root", hashed_address = ?self.hashed_address, "calculating storage root");

        let mut hashed_storage_cursor = self.hashed_cursor_factory.hashed_storage_cursor()?;

        let mut trie_cursor = StorageTrieCursor::new(
            self.tx.cursor_dup_read::<tables::StoragesTrie>()?,
            self.hashed_address,
        );

        // short circuit on empty storage
        if hashed_storage_cursor.is_empty(self.hashed_address)? {
            return Ok((
                EMPTY_ROOT,
                TrieUpdates::from([(TrieKey::StorageTrie(self.hashed_address), TrieOp::Delete)]),
            ))
        }

        let mut walker = TrieWalker::new(&mut trie_cursor, self.changed_prefixes.clone())
            .with_updates(retain_updates);

        let mut hash_builder = HashBuilder::default().with_updates(retain_updates);

        while let Some(key) = walker.key() {
            if walker.can_skip_current_node {
                hash_builder.add_branch(key, walker.hash().unwrap(), walker.children_are_in_trie());
            }

            let seek_key = match walker.next_unprocessed_key() {
                Some(key) => key,
                None => break, // no more keys
            };

            let next_key = walker.advance()?;
            let mut storage = hashed_storage_cursor.seek(self.hashed_address, seek_key)?;
            while let Some(StorageEntry { key: hashed_key, value }) = storage {
                let storage_key_nibbles = Nibbles::unpack(hashed_key);
                if let Some(ref key) = next_key {
                    if key < &storage_key_nibbles {
                        break
                    }
                }
                hash_builder
                    .add_leaf(storage_key_nibbles, reth_rlp::encode_fixed_size(&value).as_ref());
                storage = hashed_storage_cursor.next()?;
            }
        }

        let root = hash_builder.root();

        let (_, hash_builder_updates) = hash_builder.split();
        let (_, walker_updates) = walker.split();

        let mut trie_updates = TrieUpdates::default();
        trie_updates.extend(walker_updates.into_iter());
        trie_updates.extend_with_storage_updates(self.hashed_address, hash_builder_updates);

        tracing::debug!(target: "trie::storage_root", ?root, hashed_address = ?self.hashed_address, "calculated storage root");
        Ok((root, trie_updates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        state_root, state_root_prehashed, storage_root, storage_root_prehashed,
    };
    use proptest::{prelude::ProptestConfig, proptest};
    use reth_db::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
        mdbx::{test_utils::create_test_rw_db, Env, WriteMap},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives::{
        hex_literal::hex,
        keccak256,
        proofs::KeccakHasher,
        trie::{BranchNodeCompact, TrieMask},
        Account, Address, H256, U256,
    };
    use reth_provider::Transaction;
    use std::{
        collections::BTreeMap,
        ops::{Deref, DerefMut, Mul},
        str::FromStr,
    };

    fn insert_account<'a, TX: DbTxMut<'a>>(
        tx: &mut TX,
        address: Address,
        account: Account,
        storage: &BTreeMap<H256, U256>,
    ) {
        let hashed_address = keccak256(address);
        tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();
        insert_storage(tx, hashed_address, storage);
    }

    fn insert_storage<'a, TX: DbTxMut<'a>>(
        tx: &mut TX,
        hashed_address: H256,
        storage: &BTreeMap<H256, U256>,
    ) {
        for (k, v) in storage {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: *v },
            )
            .unwrap();
        }
    }

    fn incremental_vs_full_root(inputs: &[&str], modified: &str) {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let hashed_address = H256::from_low_u64_be(1);

        let mut hashed_storage_cursor = tx.cursor_dup_write::<tables::HashedStorage>().unwrap();
        let data = inputs.iter().map(|x| H256::from_str(x).unwrap());
        let value = U256::from(0);
        for key in data {
            hashed_storage_cursor.upsert(hashed_address, StorageEntry { key, value }).unwrap();
        }

        // Generate the intermediate nodes on the receiving end of the channel
        let (_, trie_updates) =
            StorageRoot::new_hashed(tx.deref(), hashed_address).root_with_updates().unwrap();

        // 1. Some state transition happens, update the hashed storage to the new value
        let modified_key = H256::from_str(modified).unwrap();
        let value = U256::from(1);
        if hashed_storage_cursor.seek_by_key_subkey(hashed_address, modified_key).unwrap().is_some()
        {
            hashed_storage_cursor.delete_current().unwrap();
        }
        hashed_storage_cursor
            .upsert(hashed_address, StorageEntry { key: modified_key, value })
            .unwrap();

        // 2. Calculate full merkle root
        let loader = StorageRoot::new_hashed(tx.deref(), hashed_address);
        let modified_root = loader.root().unwrap();

        // Update the intermediate roots table so that we can run the incremental verification
        trie_updates.flush(tx.deref()).unwrap();

        // 3. Calculate the incremental root
        let mut storage_changes = PrefixSet::default();
        storage_changes.insert(Nibbles::unpack(modified_key));
        let loader = StorageRoot::new_hashed(tx.deref_mut(), hashed_address)
            .with_changed_prefixes(storage_changes);
        let incremental_root = loader.root().unwrap();

        assert_eq!(modified_root, incremental_root);
    }

    #[test]
    // TODO: Try to find the edge case by creating some more very complex trie.
    fn branch_node_child_changes() {
        incremental_vs_full_root(
            &[
                "1000000000000000000000000000000000000000000000000000000000000000",
                "1100000000000000000000000000000000000000000000000000000000000000",
                "1110000000000000000000000000000000000000000000000000000000000000",
                "1200000000000000000000000000000000000000000000000000000000000000",
                "1220000000000000000000000000000000000000000000000000000000000000",
                "1320000000000000000000000000000000000000000000000000000000000000",
            ],
            "1200000000000000000000000000000000000000000000000000000000000000",
        );
    }

    #[test]
    fn arbitrary_storage_root() {
        proptest!(ProptestConfig::with_cases(10), |(item: (Address, std::collections::BTreeMap<H256, U256>))| {
            let (address, storage) = item;

            let hashed_address = keccak256(address);
            let db = create_test_rw_db();
            let mut tx = Transaction::new(db.as_ref()).unwrap();
            for (key, value) in &storage {
                tx.put::<tables::HashedStorage>(
                    hashed_address,
                    StorageEntry { key: keccak256(key), value: *value },
                )
                .unwrap();
            }
            tx.commit().unwrap();

            let got = StorageRoot::new(tx.deref_mut(), address).root().unwrap();
            let expected = storage_root(storage.into_iter());
            assert_eq!(expected, got);
        });
    }

    #[test]
    // This ensures we dont add empty accounts to the trie
    fn test_empty_account() {
        let state: State = BTreeMap::from([
            (
                Address::random(),
                (
                    Account { nonce: 0, balance: U256::from(0), bytecode_hash: None },
                    BTreeMap::from([(H256::from_low_u64_be(0x4), U256::from(12))]),
                ),
            ),
            (
                Address::random(),
                (
                    Account { nonce: 0, balance: U256::from(0), bytecode_hash: None },
                    BTreeMap::default(),
                ),
            ),
            (
                Address::random(),
                (
                    Account {
                        nonce: 155,
                        balance: U256::from(414241124u32),
                        bytecode_hash: Some(keccak256("test")),
                    },
                    BTreeMap::from([
                        (H256::zero(), U256::from(3)),
                        (H256::from_low_u64_be(2), U256::from(1)),
                    ]),
                ),
            ),
        ]);
        test_state_root_with_state(state);
    }

    #[test]
    // This ensures we return an empty root when there are no storage entries
    fn test_empty_storage_root() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::random();
        let code = "el buen fla";
        let account = Account {
            nonce: 155,
            balance: U256::from(414241124u32),
            bytecode_hash: Some(keccak256(code)),
        };
        insert_account(&mut *tx, address, account, &Default::default());
        tx.commit().unwrap();

        let got = StorageRoot::new(tx.deref_mut(), address).root().unwrap();
        assert_eq!(got, EMPTY_ROOT);
    }

    #[test]
    // This ensures that the walker goes over all the storage slots
    fn test_storage_root() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::random();
        let storage = BTreeMap::from([
            (H256::zero(), U256::from(3)),
            (H256::from_low_u64_be(2), U256::from(1)),
        ]);

        let code = "el buen fla";
        let account = Account {
            nonce: 155,
            balance: U256::from(414241124u32),
            bytecode_hash: Some(keccak256(code)),
        };

        insert_account(&mut *tx, address, account, &storage);
        tx.commit().unwrap();

        let got = StorageRoot::new(tx.deref_mut(), address).root().unwrap();

        assert_eq!(storage_root(storage.into_iter()), got);
    }

    type State = BTreeMap<Address, (Account, BTreeMap<H256, U256>)>;

    #[test]
    fn arbitrary_state_root() {
        proptest!(
            ProptestConfig::with_cases(10), | (state: State) | {
                test_state_root_with_state(state);
            }
        );
    }

    #[test]
    fn arbitrary_state_root_with_progress() {
        proptest!(
            ProptestConfig::with_cases(10), | (state: State) | {
                let db = create_test_rw_db();
                let mut tx = Transaction::new(db.as_ref()).unwrap();

                for (address, (account, storage)) in &state {
                    insert_account(&mut *tx, *address, *account, storage)
                }
                tx.commit().unwrap();
                let expected = state_root(state.into_iter());

                let threshold = 10;
                let mut got = None;

                let mut intermediate_state: Option<Box<IntermediateStateRootState>> = None;
                while got.is_none() {
                    let calculator = StateRoot::new(tx.deref_mut())
                        .with_threshold(threshold)
                        .with_intermediate_state(intermediate_state.take().map(|state| *state));
                    match calculator.root_with_progress().unwrap() {
                        StateRootProgress::Progress(state, _updates) => intermediate_state = Some(state),
                        StateRootProgress::Complete(root, _updates) => got = Some(root),
                    };
                }
                assert_eq!(expected, got.unwrap());
            }
        );
    }

    fn test_state_root_with_state(state: State) {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        for (address, (account, storage)) in &state {
            insert_account(&mut *tx, *address, *account, storage)
        }
        tx.commit().unwrap();
        let expected = state_root(state.into_iter());

        let got = StateRoot::new(tx.deref_mut()).root().unwrap();
        assert_eq!(expected, got);
    }

    fn encode_account(account: Account, storage_root: Option<H256>) -> Vec<u8> {
        let mut account = EthAccount::from(account);
        if let Some(storage_root) = storage_root {
            account = account.with_storage_root(storage_root);
        }
        let mut account_rlp = Vec::with_capacity(account.length());
        account.encode(&mut account_rlp);
        account_rlp
    }

    #[test]
    fn storage_root_regression() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        // Some address whose hash starts with 0xB041
        let address3 = Address::from_str("16b07afd1c635f77172e842a000ead9a2a222459").unwrap();
        let key3 = keccak256(address3);
        assert_eq!(key3[0], 0xB0);
        assert_eq!(key3[1], 0x41);

        let storage = BTreeMap::from(
            [
                ("1200000000000000000000000000000000000000000000000000000000000000", 0x42),
                ("1400000000000000000000000000000000000000000000000000000000000000", 0x01),
                ("3000000000000000000000000000000000000000000000000000000000E00000", 0x127a89),
                ("3000000000000000000000000000000000000000000000000000000000E00001", 0x05),
            ]
            .map(|(slot, val)| (H256::from_str(slot).unwrap(), U256::from(val))),
        );

        let mut hashed_storage_cursor = tx.cursor_dup_write::<tables::HashedStorage>().unwrap();
        for (hashed_slot, value) in storage.clone() {
            hashed_storage_cursor.upsert(key3, StorageEntry { key: hashed_slot, value }).unwrap();
        }
        tx.commit().unwrap();

        let account3_storage_root = StorageRoot::new(tx.deref_mut(), address3).root().unwrap();
        let expected_root = storage_root_prehashed(storage.into_iter());
        assert_eq!(expected_root, account3_storage_root);
    }

    #[test]
    fn account_and_storage_trie() {
        let ether = U256::from(1e18);
        let storage = BTreeMap::from(
            [
                ("1200000000000000000000000000000000000000000000000000000000000000", 0x42),
                ("1400000000000000000000000000000000000000000000000000000000000000", 0x01),
                ("3000000000000000000000000000000000000000000000000000000000E00000", 0x127a89),
                ("3000000000000000000000000000000000000000000000000000000000E00001", 0x05),
            ]
            .map(|(slot, val)| (H256::from_str(slot).unwrap(), U256::from(val))),
        );

        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let mut hashed_account_cursor = tx.cursor_write::<tables::HashedAccount>().unwrap();
        let mut hashed_storage_cursor = tx.cursor_dup_write::<tables::HashedStorage>().unwrap();

        let mut hash_builder = HashBuilder::default();

        // Insert first account
        let key1 =
            H256::from_str("b000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();
        let account1 = Account { nonce: 0, balance: U256::from(3).mul(ether), bytecode_hash: None };
        hashed_account_cursor.upsert(key1, account1).unwrap();
        hash_builder.add_leaf(Nibbles::unpack(key1), &encode_account(account1, None));

        // Some address whose hash starts with 0xB040
        let address2 = Address::from_str("7db3e81b72d2695e19764583f6d219dbee0f35ca").unwrap();
        let key2 = keccak256(address2);
        assert_eq!(key2[0], 0xB0);
        assert_eq!(key2[1], 0x40);
        let account2 = Account { nonce: 0, balance: ether, ..Default::default() };
        hashed_account_cursor.upsert(key2, account2).unwrap();
        hash_builder.add_leaf(Nibbles::unpack(key2), &encode_account(account2, None));

        // Some address whose hash starts with 0xB041
        let address3 = Address::from_str("16b07afd1c635f77172e842a000ead9a2a222459").unwrap();
        let key3 = keccak256(address3);
        assert_eq!(key3[0], 0xB0);
        assert_eq!(key3[1], 0x41);
        let code_hash =
            H256::from_str("5be74cad16203c4905c068b012a2e9fb6d19d036c410f16fd177f337541440dd")
                .unwrap();
        let account3 =
            Account { nonce: 0, balance: U256::from(2).mul(ether), bytecode_hash: Some(code_hash) };
        hashed_account_cursor.upsert(key3, account3).unwrap();
        for (hashed_slot, value) in storage {
            if hashed_storage_cursor
                .seek_by_key_subkey(key3, hashed_slot)
                .unwrap()
                .filter(|e| e.key == hashed_slot)
                .is_some()
            {
                hashed_storage_cursor.delete_current().unwrap();
            }
            hashed_storage_cursor.upsert(key3, StorageEntry { key: hashed_slot, value }).unwrap();
        }
        let account3_storage_root = StorageRoot::new(tx.deref_mut(), address3).root().unwrap();
        hash_builder.add_leaf(
            Nibbles::unpack(key3),
            &encode_account(account3, Some(account3_storage_root)),
        );

        let key4a =
            H256::from_str("B1A0000000000000000000000000000000000000000000000000000000000000")
                .unwrap();
        let account4a =
            Account { nonce: 0, balance: U256::from(4).mul(ether), ..Default::default() };
        hashed_account_cursor.upsert(key4a, account4a).unwrap();
        hash_builder.add_leaf(Nibbles::unpack(key4a), &encode_account(account4a, None));

        let key5 =
            H256::from_str("B310000000000000000000000000000000000000000000000000000000000000")
                .unwrap();
        let account5 =
            Account { nonce: 0, balance: U256::from(8).mul(ether), ..Default::default() };
        hashed_account_cursor.upsert(key5, account5).unwrap();
        hash_builder.add_leaf(Nibbles::unpack(key5), &encode_account(account5, None));

        let key6 =
            H256::from_str("B340000000000000000000000000000000000000000000000000000000000000")
                .unwrap();
        let account6 =
            Account { nonce: 0, balance: U256::from(1).mul(ether), ..Default::default() };
        hashed_account_cursor.upsert(key6, account6).unwrap();
        hash_builder.add_leaf(Nibbles::unpack(key6), &encode_account(account6, None));

        // Populate account & storage trie DB tables
        let expected_root =
            H256::from_str("72861041bc90cd2f93777956f058a545412b56de79af5eb6b8075fe2eabbe015")
                .unwrap();
        let computed_expected_root: H256 = triehash::trie_root::<KeccakHasher, _, _, _>([
            (key1, encode_account(account1, None)),
            (key2, encode_account(account2, None)),
            (key3, encode_account(account3, Some(account3_storage_root))),
            (key4a, encode_account(account4a, None)),
            (key5, encode_account(account5, None)),
            (key6, encode_account(account6, None)),
        ]);
        // Check computed trie root to ensure correctness
        assert_eq!(computed_expected_root, expected_root);

        // Check hash builder root
        assert_eq!(hash_builder.root(), computed_expected_root);

        // Check state root calculation from scratch
        let (root, trie_updates) = StateRoot::new(tx.deref()).root_with_updates().unwrap();
        assert_eq!(root, computed_expected_root);

        // Check account trie
        let mut account_updates = trie_updates
            .iter()
            .filter_map(|(k, v)| match (k, v) {
                (TrieKey::AccountNode(nibbles), TrieOp::Update(node)) => Some((nibbles, node)),
                _ => None,
            })
            .collect::<Vec<_>>();
        account_updates.sort_unstable_by(|a, b| a.0.cmp(b.0));
        assert_eq!(account_updates.len(), 2);

        let (nibbles1a, node1a) = account_updates.first().unwrap();
        assert_eq!(nibbles1a.inner[..], [0xB]);
        assert_eq!(node1a.state_mask, TrieMask::new(0b1011));
        assert_eq!(node1a.tree_mask, TrieMask::new(0b0001));
        assert_eq!(node1a.hash_mask, TrieMask::new(0b1001));
        assert_eq!(node1a.root_hash, None);
        assert_eq!(node1a.hashes.len(), 2);

        let (nibbles2a, node2a) = account_updates.last().unwrap();
        assert_eq!(nibbles2a.inner[..], [0xB, 0x0]);
        assert_eq!(node2a.state_mask, TrieMask::new(0b10001));
        assert_eq!(node2a.tree_mask, TrieMask::new(0b00000));
        assert_eq!(node2a.hash_mask, TrieMask::new(0b10000));
        assert_eq!(node2a.root_hash, None);
        assert_eq!(node2a.hashes.len(), 1);

        // Check storage trie
        let storage_updates = trie_updates
            .iter()
            .filter_map(|entry| match entry {
                (TrieKey::StorageNode(_, nibbles), TrieOp::Update(node)) => Some((nibbles, node)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(storage_updates.len(), 1);

        let (nibbles3, node3) = storage_updates.first().unwrap();
        assert!(nibbles3.inner.is_empty());
        assert_eq!(node3.state_mask, TrieMask::new(0b1010));
        assert_eq!(node3.tree_mask, TrieMask::new(0b0000));
        assert_eq!(node3.hash_mask, TrieMask::new(0b0010));

        assert_eq!(node3.hashes.len(), 1);
        assert_eq!(node3.root_hash, Some(account3_storage_root));

        // Add an account
        // Some address whose hash starts with 0xB1
        let address4b = Address::from_str("4f61f2d5ebd991b85aa1677db97307caf5215c91").unwrap();
        let key4b = keccak256(address4b);
        assert_eq!(key4b.0[0], key4a.0[0]);
        let account4b =
            Account { nonce: 0, balance: U256::from(5).mul(ether), bytecode_hash: None };
        hashed_account_cursor.upsert(key4b, account4b).unwrap();

        let mut prefix_set = PrefixSet::default();
        prefix_set.insert(Nibbles::unpack(key4b));

        let expected_state_root =
            H256::from_str("8e263cd4eefb0c3cbbb14e5541a66a755cad25bcfab1e10dd9d706263e811b28")
                .unwrap();

        let (root, trie_updates) = StateRoot::new(tx.deref())
            .with_changed_account_prefixes(prefix_set)
            .root_with_updates()
            .unwrap();
        assert_eq!(root, expected_state_root);

        let mut account_updates = trie_updates
            .iter()
            .filter_map(|entry| match entry {
                (TrieKey::AccountNode(nibbles), TrieOp::Update(node)) => Some((nibbles, node)),
                _ => None,
            })
            .collect::<Vec<_>>();
        account_updates.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(account_updates.len(), 2);

        let (nibbles1b, node1b) = account_updates.first().unwrap();
        assert_eq!(nibbles1b.inner[..], [0xB]);
        assert_eq!(node1b.state_mask, TrieMask::new(0b1011));
        assert_eq!(node1b.tree_mask, TrieMask::new(0b0001));
        assert_eq!(node1b.hash_mask, TrieMask::new(0b1011));
        assert_eq!(node1b.root_hash, None);
        assert_eq!(node1b.hashes.len(), 3);
        assert_eq!(node1a.hashes[0], node1b.hashes[0]);
        assert_eq!(node1a.hashes[1], node1b.hashes[2]);

        let (nibbles2b, node2b) = account_updates.last().unwrap();
        assert_eq!(nibbles2b.inner[..], [0xB, 0x0]);
        assert_eq!(node2a, node2b);
        tx.commit().unwrap();

        {
            let mut hashed_account_cursor = tx.cursor_write::<tables::HashedAccount>().unwrap();

            let account = hashed_account_cursor.seek_exact(key2).unwrap().unwrap();
            hashed_account_cursor.delete_current().unwrap();

            let mut account_prefix_set = PrefixSet::default();
            account_prefix_set.insert(Nibbles::unpack(account.0));

            let computed_expected_root: H256 = triehash::trie_root::<KeccakHasher, _, _, _>([
                (key1, encode_account(account1, None)),
                // DELETED: (key2, encode_account(account2, None)),
                (key3, encode_account(account3, Some(account3_storage_root))),
                (key4a, encode_account(account4a, None)),
                (key4b, encode_account(account4b, None)),
                (key5, encode_account(account5, None)),
                (key6, encode_account(account6, None)),
            ]);

            let (root, trie_updates) = StateRoot::new(tx.deref())
                .with_changed_account_prefixes(account_prefix_set)
                .root_with_updates()
                .unwrap();
            assert_eq!(root, computed_expected_root);
            assert_eq!(trie_updates.len(), 7);
            assert_eq!(trie_updates.iter().filter(|(_, op)| op.is_update()).count(), 2);

            let account_updates = trie_updates
                .iter()
                .filter_map(|entry| match entry {
                    (TrieKey::AccountNode(nibbles), TrieOp::Update(node)) => Some((nibbles, node)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(account_updates.len(), 1);

            let (nibbles1c, node1c) = account_updates.first().unwrap();
            assert_eq!(nibbles1c.inner[..], [0xB]);

            assert_eq!(node1c.state_mask, TrieMask::new(0b1011));
            assert_eq!(node1c.tree_mask, TrieMask::new(0b0000));
            assert_eq!(node1c.hash_mask, TrieMask::new(0b1011));

            assert_eq!(node1c.root_hash, None);

            assert_eq!(node1c.hashes.len(), 3);
            assert_ne!(node1c.hashes[0], node1b.hashes[0]);
            assert_eq!(node1c.hashes[1], node1b.hashes[1]);
            assert_eq!(node1c.hashes[2], node1b.hashes[2]);
            tx.drop().unwrap();
        }

        {
            let mut hashed_account_cursor = tx.cursor_write::<tables::HashedAccount>().unwrap();

            let account2 = hashed_account_cursor.seek_exact(key2).unwrap().unwrap();
            hashed_account_cursor.delete_current().unwrap();
            let account3 = hashed_account_cursor.seek_exact(key3).unwrap().unwrap();
            hashed_account_cursor.delete_current().unwrap();

            let mut account_prefix_set = PrefixSet::default();
            account_prefix_set.insert(Nibbles::unpack(account2.0));
            account_prefix_set.insert(Nibbles::unpack(account3.0));

            let computed_expected_root: H256 = triehash::trie_root::<KeccakHasher, _, _, _>([
                (key1, encode_account(account1, None)),
                // DELETED: (key2, encode_account(account2, None)),
                // DELETED: (key3, encode_account(account3, Some(account3_storage_root))),
                (key4a, encode_account(account4a, None)),
                (key4b, encode_account(account4b, None)),
                (key5, encode_account(account5, None)),
                (key6, encode_account(account6, None)),
            ]);

            let (root, trie_updates) = StateRoot::new(tx.deref_mut())
                .with_changed_account_prefixes(account_prefix_set)
                .root_with_updates()
                .unwrap();
            assert_eq!(root, computed_expected_root);
            assert_eq!(trie_updates.len(), 6);
            assert_eq!(trie_updates.iter().filter(|(_, op)| op.is_update()).count(), 1); // no storage root update

            let account_updates = trie_updates
                .iter()
                .filter_map(|entry| match entry {
                    (TrieKey::AccountNode(nibbles), TrieOp::Update(node)) => Some((nibbles, node)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(account_updates.len(), 1);

            let (nibbles1d, node1d) = account_updates.first().unwrap();
            assert_eq!(nibbles1d.inner[..], [0xB]);

            assert_eq!(node1d.state_mask, TrieMask::new(0b1011));
            assert_eq!(node1d.tree_mask, TrieMask::new(0b0000));
            assert_eq!(node1d.hash_mask, TrieMask::new(0b1010));

            assert_eq!(node1d.root_hash, None);

            assert_eq!(node1d.hashes.len(), 2);
            assert_eq!(node1d.hashes[0], node1b.hashes[1]);
            assert_eq!(node1d.hashes[1], node1b.hashes[2]);
        }
    }

    #[test]
    fn account_trie_around_extension_node() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let expected = extension_node_trie(&mut tx);

        let (got, updates) = StateRoot::new(tx.deref_mut()).root_with_updates().unwrap();
        assert_eq!(expected, got);

        // Check account trie
        let account_updates = updates
            .iter()
            .filter_map(|entry| match entry {
                (TrieKey::AccountNode(nibbles), TrieOp::Update(node)) => {
                    Some((nibbles.inner[..].into(), node.clone()))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();

        assert_trie_updates(&account_updates);
    }

    #[test]

    fn account_trie_around_extension_node_with_dbtrie() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let expected = extension_node_trie(&mut tx);

        let (got, updates) = StateRoot::new(tx.deref_mut()).root_with_updates().unwrap();
        assert_eq!(expected, got);
        updates.flush(tx.deref_mut()).unwrap();

        // read the account updates from the db
        let mut accounts_trie = tx.cursor_read::<tables::AccountsTrie>().unwrap();
        let walker = accounts_trie.walk(None).unwrap();
        let mut account_updates = HashMap::new();
        for item in walker {
            let (key, node) = item.unwrap();
            account_updates.insert(key.inner[..].into(), node);
        }

        assert_trie_updates(&account_updates);
    }

    // TODO: limit the thumber of test cases?
    proptest! {
        #[test]
        fn fuzz_state_root_incremental(account_changes: [BTreeMap<H256, U256>; 5]) {
            tokio::runtime::Runtime::new().unwrap().block_on(async {

                let db = create_test_rw_db();
                let mut tx = Transaction::new(db.as_ref()).unwrap();
                let mut hashed_account_cursor = tx.cursor_write::<tables::HashedAccount>().unwrap();

                let mut state = BTreeMap::default();
                for accounts in account_changes {
                    let should_generate_changeset = !state.is_empty();
                    let mut changes = PrefixSet::default();
                    for (hashed_address, balance) in accounts.clone() {
                        hashed_account_cursor.upsert(hashed_address, Account { balance,..Default::default() }).unwrap();
                        if should_generate_changeset {
                            changes.insert(Nibbles::unpack(hashed_address));
                        }
                    }

                    let (state_root, trie_updates) = StateRoot::new(tx.deref_mut())
                        .with_changed_account_prefixes(changes)
                        .root_with_updates()
                        .unwrap();

                    state.append(&mut accounts.clone());
                    let expected_root = state_root_prehashed(
                        state.clone().into_iter().map(|(key, balance)| (key, (Account { balance, ..Default::default() }, std::iter::empty())))
                    );
                    assert_eq!(expected_root, state_root);
                    trie_updates.flush(tx.deref_mut()).unwrap();
                }
            });
        }
    }

    #[test]
    fn storage_trie_around_extension_node() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let hashed_address = H256::random();
        let (expected_root, expected_updates) =
            extension_node_storage_trie(&mut tx, hashed_address);

        let (got, updates) =
            StorageRoot::new_hashed(tx.deref_mut(), hashed_address).root_with_updates().unwrap();
        assert_eq!(expected_root, got);

        // Check account trie
        let storage_updates = updates
            .iter()
            .filter_map(|entry| match entry {
                (TrieKey::StorageNode(_, nibbles), TrieOp::Update(node)) => {
                    Some((nibbles.inner[..].into(), node.clone()))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(expected_updates, storage_updates);

        assert_trie_updates(&storage_updates);
    }

    fn extension_node_storage_trie(
        tx: &mut Transaction<'_, Env<WriteMap>>,
        hashed_address: H256,
    ) -> (H256, HashMap<Nibbles, BranchNodeCompact>) {
        let value = U256::from(1);

        let mut hashed_storage = tx.cursor_write::<tables::HashedStorage>().unwrap();

        let mut hb = HashBuilder::default().with_updates(true);

        for key in [
            hex!("30af561000000000000000000000000000000000000000000000000000000000"),
            hex!("30af569000000000000000000000000000000000000000000000000000000000"),
            hex!("30af650000000000000000000000000000000000000000000000000000000000"),
            hex!("30af6f0000000000000000000000000000000000000000000000000000000000"),
            hex!("30af8f0000000000000000000000000000000000000000000000000000000000"),
            hex!("3100000000000000000000000000000000000000000000000000000000000000"),
        ] {
            hashed_storage.upsert(hashed_address, StorageEntry { key: H256(key), value }).unwrap();
            hb.add_leaf(Nibbles::unpack(key), &reth_rlp::encode_fixed_size(&value));
        }

        let root = hb.root();
        let (_, updates) = hb.split();
        (root, updates)
    }

    fn extension_node_trie(tx: &mut Transaction<'_, Env<WriteMap>>) -> H256 {
        let a =
            Account { nonce: 0, balance: U256::from(1u64), bytecode_hash: Some(H256::random()) };
        let val = encode_account(a, None);

        let mut hashed_accounts = tx.cursor_write::<tables::HashedAccount>().unwrap();
        let mut hb = HashBuilder::default();

        for key in [
            hex!("30af561000000000000000000000000000000000000000000000000000000000"),
            hex!("30af569000000000000000000000000000000000000000000000000000000000"),
            hex!("30af650000000000000000000000000000000000000000000000000000000000"),
            hex!("30af6f0000000000000000000000000000000000000000000000000000000000"),
            hex!("30af8f0000000000000000000000000000000000000000000000000000000000"),
            hex!("3100000000000000000000000000000000000000000000000000000000000000"),
        ] {
            hashed_accounts.upsert(H256(key), a).unwrap();
            hb.add_leaf(Nibbles::unpack(key), &val);
        }

        hb.root()
    }

    fn assert_trie_updates(account_updates: &HashMap<Nibbles, BranchNodeCompact>) {
        assert_eq!(account_updates.len(), 2);

        let node = account_updates.get(&vec![0x3].into()).unwrap();
        let expected = BranchNodeCompact::new(0b0011, 0b0001, 0b0000, vec![], None);
        assert_eq!(node, &expected);

        let node = account_updates.get(&vec![0x3, 0x0, 0xA, 0xF].into()).unwrap();
        assert_eq!(node.state_mask, TrieMask::new(0b101100000));
        assert_eq!(node.tree_mask, TrieMask::new(0b000000000));
        assert_eq!(node.hash_mask, TrieMask::new(0b001000000));

        assert_eq!(node.root_hash, None);
        assert_eq!(node.hashes.len(), 1);
    }
}
