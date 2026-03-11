use alloy_primitives::{Address, B256, U256};
use alloy_rlp::encode_fixed_size;
use reth_primitives_traits::Account;
use reth_trie_common::triehash::KeccakHasher;

/// Re-export of [triehash].
pub use triehash;

/// Compute the state root of a given set of accounts using [`triehash::sec_trie_root`].
pub fn state_root<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (Address, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let encoded_accounts = accounts.into_iter().map(|(address, (account, storage))| {
        let storage_root = storage_root(storage);
        let account = account.into_trie_account(storage_root);
        (address, alloy_rlp::encode(account))
    });
    triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_accounts)
}

/// Compute the storage root for a given account using [`triehash::sec_trie_root`].
pub fn storage_root<I: IntoIterator<Item = (B256, U256)>>(storage: I) -> B256 {
    let encoded_storage = storage.into_iter().map(|(k, v)| (k, encode_fixed_size(&v)));
    triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage)
}

/// Compute the state root of a given set of accounts with prehashed keys using
/// [`triehash::trie_root`].
pub fn state_root_prehashed<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (B256, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let encoded_accounts = accounts.into_iter().map(|(address, (account, storage))| {
        let storage_root = storage_root_prehashed(storage);
        let account = account.into_trie_account(storage_root);
        (address, alloy_rlp::encode(account))
    });

    triehash::trie_root::<KeccakHasher, _, _, _>(encoded_accounts)
}

/// Compute the storage root for a given account with prehashed slots using [`triehash::trie_root`].
pub fn storage_root_prehashed<I: IntoIterator<Item = (B256, U256)>>(storage: I) -> B256 {
    let encoded_storage = storage.into_iter().map(|(k, v)| (k, encode_fixed_size(&v)));
    triehash::trie_root::<KeccakHasher, _, _, _>(encoded_storage)
}

// ---------------------------------------------------------------------------
// Trie test harness
// ---------------------------------------------------------------------------

use crate::{
    hashed_cursor::{
        mock::MockHashedCursorFactory, HashedCursorFactory, HashedPostStateCursorFactory,
    },
    proof_v2::StorageProofCalculator,
    trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
    StorageRoot,
};
use alloy_primitives::map::HashSet;
use reth_trie_common::{
    prefix_set::PrefixSetMut, updates::StorageTrieUpdates, BranchNodeCompact,
    HashedPostStateSorted, HashedStorage, Nibbles, ProofTrieNodeV2, ProofV2Target,
};
use std::{collections::BTreeMap, iter::once};

/// General-purpose test harness for storage trie tests.
///
/// Manages a base storage dataset, computes expected roots via [`StorageRoot`], and generates
/// V2 proofs via [`StorageProofCalculator`] using mock cursors.
#[derive(Debug)]
pub struct TrieTestHarness {
    /// The base storage dataset (hashed slot → value). Zero-valued entries are absent.
    storage: BTreeMap<B256, U256>,
    /// The expected storage root, calculated by [`StorageRoot`].
    original_root: B256,
    /// The starting storage trie updates, used for minimization.
    storage_trie_updates: StorageTrieUpdates,
    /// Mock factory for trie cursors.
    trie_cursor_factory: MockTrieCursorFactory,
    /// Mock factory for hashed cursors.
    hashed_cursor_factory: MockHashedCursorFactory,
}

impl TrieTestHarness {
    /// Creates a new test harness from a map of hashed storage slots to values.
    pub fn new(storage: BTreeMap<B256, U256>) -> Self {
        let mut harness = Self {
            storage,
            original_root: B256::ZERO,
            storage_trie_updates: StorageTrieUpdates::default(),
            trie_cursor_factory: MockTrieCursorFactory::new(
                BTreeMap::new(),
                once((B256::ZERO, BTreeMap::new())).collect(),
            ),
            hashed_cursor_factory: MockHashedCursorFactory::new(
                BTreeMap::new(),
                once((B256::ZERO, BTreeMap::new())).collect(),
            ),
        };
        harness.rebuild();
        harness
    }

    /// Computes the storage root and trie updates after applying the given changeset on top
    /// of the current base storage.
    ///
    /// Builds a [`HashedPostStateCursorFactory`] overlay, derives a prefix set from the
    /// changeset keys, and passes both into [`StorageRoot::new_hashed`].
    pub fn get_root_with_updates(
        &self,
        changeset: &BTreeMap<B256, U256>,
    ) -> (B256, StorageTrieUpdates) {
        let mut prefix_set = PrefixSetMut::with_capacity(changeset.len());
        for hashed_slot in changeset.keys() {
            prefix_set.insert(Nibbles::unpack(hashed_slot));
        }

        let hashed_storage =
            HashedStorage::from_iter(false, changeset.iter().map(|(&k, &v)| (k, v)));
        let overlay = HashedPostStateSorted::new(
            Vec::new(),
            once((self.hashed_address(), hashed_storage.into_sorted())).collect(),
        );
        let overlay_cursor_factory =
            HashedPostStateCursorFactory::new(self.hashed_cursor_factory.clone(), &overlay);

        let (root, _, updates) = StorageRoot::new_hashed(
            self.trie_cursor_factory.clone(),
            overlay_cursor_factory,
            self.hashed_address(),
            prefix_set.freeze(),
            #[cfg(feature = "metrics")]
            crate::metrics::TrieRootMetrics::new(crate::TrieType::Storage),
        )
        .root_with_updates()
        .expect("StorageRoot should succeed");

        (root, updates)
    }

    /// Merges `changeset` into the base storage (zero values remove entries) and
    /// rebuilds the harness from scratch with the resulting storage.
    pub fn apply_changeset(&mut self, changeset: BTreeMap<B256, U256>) {
        for (k, v) in changeset {
            if v == U256::ZERO {
                self.storage.remove(&k);
            } else {
                self.storage.insert(k, v);
            }
        }
        self.rebuild();
    }

    /// Recomputes the storage root, trie updates, and cursor factories from `self.storage`.
    fn rebuild(&mut self) {
        self.hashed_cursor_factory = MockHashedCursorFactory::new(
            BTreeMap::new(),
            once((self.hashed_address(), self.storage.clone())).collect(),
        );

        let (root, _, updates) = StorageRoot::new_hashed(
            MockTrieCursorFactory::new(
                BTreeMap::new(),
                once((self.hashed_address(), BTreeMap::new())).collect(),
            ),
            self.hashed_cursor_factory.clone(),
            self.hashed_address(),
            crate::prefix_set::PrefixSet::default(),
            #[cfg(feature = "metrics")]
            crate::metrics::TrieRootMetrics::new(crate::TrieType::Storage),
        )
        .root_with_updates()
        .expect("StorageRoot should succeed");

        self.trie_cursor_factory = MockTrieCursorFactory::new(
            BTreeMap::new(),
            once((
                self.hashed_address(),
                updates.storage_nodes.iter().map(|(k, v)| (*k, v.clone())).collect(),
            ))
            .collect(),
        );

        self.original_root = root;
        self.storage_trie_updates = updates;
    }

    /// Returns the hashed address used for all storage trie operations.
    pub const fn hashed_address(&self) -> B256 {
        B256::ZERO
    }

    /// Returns a reference to the base storage dataset.
    pub const fn storage(&self) -> &BTreeMap<B256, U256> {
        &self.storage
    }

    /// Returns the expected storage root.
    pub const fn original_root(&self) -> B256 {
        self.original_root
    }

    /// Returns a reference to the storage trie updates.
    pub const fn storage_trie_updates(&self) -> &StorageTrieUpdates {
        &self.storage_trie_updates
    }

    /// Replaces the trie cursor factory with one backed by the given trie nodes.
    pub fn set_trie_nodes(&mut self, trie_nodes: BTreeMap<Nibbles, BranchNodeCompact>) {
        self.trie_cursor_factory = MockTrieCursorFactory::new(
            BTreeMap::new(),
            once((self.hashed_address(), trie_nodes)).collect(),
        );
    }

    /// Returns a clone of the mock trie cursor factory.
    pub fn trie_cursor_factory(&self) -> MockTrieCursorFactory {
        self.trie_cursor_factory.clone()
    }

    /// Returns a clone of the mock hashed cursor factory.
    pub fn hashed_cursor_factory(&self) -> MockHashedCursorFactory {
        self.hashed_cursor_factory.clone()
    }

    /// Obtains the root node of the storage trie via [`StorageProofCalculator`].
    pub fn root_node(&self) -> ProofTrieNodeV2 {
        let trie_cursor = self
            .trie_cursor_factory
            .storage_trie_cursor(self.hashed_address())
            .expect("storage trie cursor should succeed");
        let hashed_cursor = self
            .hashed_cursor_factory
            .hashed_storage_cursor(self.hashed_address())
            .expect("hashed storage cursor should succeed");

        let mut proof_calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);
        proof_calculator
            .storage_root_node(self.hashed_address())
            .expect("storage_root_node should succeed")
    }

    /// Generates storage proofs for the given targets using [`StorageProofCalculator`].
    ///
    /// Also computes and returns the root hash (if the proof contains a root node) by reusing
    /// the calculator after the proof call.
    pub fn proof_v2(&self, targets: &mut [ProofV2Target]) -> (Vec<ProofTrieNodeV2>, Option<B256>) {
        let trie_cursor = self
            .trie_cursor_factory
            .storage_trie_cursor(self.hashed_address())
            .expect("storage trie cursor should succeed");
        let hashed_cursor = self
            .hashed_cursor_factory
            .hashed_storage_cursor(self.hashed_address())
            .expect("hashed storage cursor should succeed");

        let mut proof_calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);
        let proofs = proof_calculator
            .storage_proof(self.hashed_address(), targets)
            .expect("proof_v2 should succeed");
        let root_hash =
            proof_calculator.compute_root_hash(&proofs).expect("compute_root_hash should succeed");
        (proofs, root_hash)
    }

    /// Removes all entries from `updates` that are redundant with the starting storage
    /// trie updates.
    ///
    /// A storage node is redundant if it exists in the starting set with the same value.
    /// A removed node is redundant if it was already absent from the starting set.
    /// The `is_deleted` flag is cleared if it matches the starting value.
    pub fn minimize_trie_updates(&self, updates: &mut StorageTrieUpdates) {
        if updates.is_deleted == self.storage_trie_updates.is_deleted {
            updates.is_deleted = false;
        }

        // StorageTrieUpdates::finalize can leave the same path in both storage_nodes
        // and removed_nodes. Per into_sorted, updated nodes take precedence over
        // removed ones. Record which paths had an update before minimization so we
        // can drop their corresponding removals.
        let paths_with_updates: HashSet<Nibbles> = updates.storage_nodes.keys().copied().collect();

        updates
            .storage_nodes
            .retain(|path, node| self.storage_trie_updates.storage_nodes.get(path) != Some(node));

        updates.removed_nodes.retain(|path| {
            self.storage_trie_updates.storage_nodes.contains_key(path) &&
                !paths_with_updates.contains(path)
        });
    }
}
