//! Account and per-account storage lifecycle checks, independent of the storage trait spine.

use super::*;
use alloy_primitives::KECCAK256_EMPTY;
use alloy_trie::{HashBuilder, TrieAccount};
use reth_trie::test_utils::triehash;
use reth_trie_common::triehash::KeccakHasher;

#[derive(Clone, Default)]
pub(super) struct ModelAccount {
    pub(super) nonce: u64,
    pub(super) balance: U256,
    pub(super) storage: BTreeMap<B256, U256>,
    pub(super) touched: BTreeSet<B256>,
}

pub(super) type Model = BTreeMap<B256, ModelAccount>;

pub(super) fn update_model(model: &mut Model, change: &Change) {
    let key = B256::from(change.key());
    match change {
        Change::Account { nonce, value, .. } => {
            let entry = model.entry(key).or_default();
            entry.nonce = *nonce;
            entry.balance = U256::from_be_bytes(*value);
        }
        Change::DeleteAccount { .. } => {
            model.remove(&key);
        }
        Change::Storage { slot, value, .. } => {
            if let Some(entry) = model.get_mut(&key) {
                let slot = B256::from(*slot);
                entry.touched.insert(slot);
                let value = U256::from_be_bytes(*value);
                if value.is_zero() {
                    entry.storage.remove(&slot);
                } else {
                    entry.storage.insert(slot, value);
                }
            }
        }
        Change::Leaf { .. } => {}
    }
}

fn encoded(account: &ModelAccount, storage_root: B256) -> Vec<u8> {
    alloy_rlp::encode(TrieAccount {
        nonce: account.nonce,
        balance: account.balance,
        storage_root,
        code_hash: KECCAK256_EMPTY,
    })
}

fn state_root(model: &Model) -> B256 {
    triehash::trie_root::<KeccakHasher, _, _, _>(
        model.iter().map(|(key, account)| (key, encoded(account, oracle(&account.storage)))),
    )
}

fn account_nodes(model: &Model) -> BTreeMap<Nibbles, reth_trie_common::BranchNodeCompact> {
    let mut builder = HashBuilder::default().with_updates(true);
    for (key, account) in model {
        builder.add_leaf(Nibbles::unpack(*key), &encoded(account, oracle(&account.storage)));
    }
    builder.root();
    // SparseTrie retained updates exclude the root's empty logical path.
    builder.split().1.into_iter().filter(|(path, _)| !path.is_empty()).collect()
}

struct State<T> {
    accounts: T,
    storages: BTreeMap<B256, T>,
    model: Model,
    deleted: BTreeSet<B256>,
}

impl<T: SparseTrie + Clone> State<T> {
    fn new(factory: &impl Fn() -> T, evidence: &mut Evidence) -> Result<Self, DifferentialFailure> {
        let mut accounts = factory();
        initialize(&mut accounts, &TrieTestHarness::new(BTreeMap::new()), true, evidence)?;
        Ok(Self {
            accounts,
            storages: BTreeMap::new(),
            model: Model::new(),
            deleted: BTreeSet::new(),
        })
    }

    fn change(
        &mut self,
        change: &Change,
        at: u64,
        factory: &impl Fn() -> T,
        evidence: &mut Evidence,
    ) -> Result<(), DifferentialFailure> {
        if matches!(change, Change::Leaf { .. }) {
            return Ok(());
        }
        let key = B256::from(change.key());
        let before_nodes = account_nodes(&self.model);
        let before = self.model.get(&key).cloned();
        let had_storage = before.as_ref().is_some_and(|account| !account.storage.is_empty());
        match change {
            Change::Account { .. } => {
                if before.is_none() {
                    let mut storage = self.storages.remove(&key).unwrap_or_else(factory);
                    // Reuse the cleared trie for a recreated identity, not a fresh replacement.
                    root(&mut storage, EMPTY_ROOT_HASH, at, evidence)?;
                    initialize(
                        &mut storage,
                        &TrieTestHarness::new(BTreeMap::new()),
                        true,
                        evidence,
                    )?;
                    self.storages.insert(key, storage);
                    if self.deleted.contains(&key) {
                        evidence.features.insert("account_recreated");
                        evidence.hit("account_recreates");
                    }
                }
                evidence.features.insert("account_metadata_update");
            }
            Change::DeleteAccount { .. } => {
                if let Some(storage) = self.storages.get_mut(&key) {
                    storage.clear();
                    evidence.hit("clear");
                    // Observe clear before initialization can replace a stale root.
                    root(storage, EMPTY_ROOT_HASH, at, evidence)?;
                    initialize(storage, &TrieTestHarness::new(BTreeMap::new()), true, evidence)?;
                }
                if before.is_some() {
                    self.deleted.insert(key);
                    evidence.features.insert("account_deleted");
                    evidence.hit("account_deletes");
                }
                if had_storage {
                    evidence.features.insert("account_storage_wiped");
                    evidence.hit("account_storage_wipes");
                }
            }
            Change::Storage { slot, value, .. } => {
                let Some(account) = &before else {
                    evidence.features.insert("storage_write_absent_account_noop");
                    return Ok(());
                };
                let harness = TrieTestHarness::new(account.storage.clone());
                let changes = BTreeMap::from([(B256::from(*slot), U256::from_be_bytes(*value))]);
                let storage = self.storages.get_mut(&key).expect("created storage trie");
                apply(storage, &harness, &mut leaf_updates(&changes), evidence)?;
                let expected = oracle(&model_after(&account.storage, &changes));
                root(storage, expected, at, evidence)?;
                let updates = retained(storage, &harness, &changes, evidence)?;
                evidence.checkpoints.push((expected, updates));
                evidence.features.insert(if value == &[0; 32] {
                    "account_storage_slot_deleted"
                } else {
                    "account_storage_written"
                });
            }
            Change::Leaf { .. } => unreachable!(),
        }
        update_model(&mut self.model, change);
        let value = if let Some(account) = self.model.get(&key) {
            let storage = self.storages.get_mut(&key).expect("created storage trie");
            let actual = storage.root(epoch(at));
            equal(actual, oracle(&account.storage), "account-storage-oracle")?;
            encoded(account, actual)
        } else {
            Vec::new()
        };
        // The account trie remains revealed; the separate trait spine and storage tries
        // exercise proof hydration and pruning without a second account-proof oracle.
        let mut pending = B256Map::from_iter([(key, LeafUpdate::Changed(value))]);
        apply_with_proofs(&mut self.accounts, &mut pending, evidence, |_, _, _| {
            Err(DifferentialFailure {
                signature: "sparse-trie/account-unexpected-blind".into(),
                details: "account trie is fully revealed".into(),
            })
        })?;
        let expected = state_root(&self.model);
        root(&mut self.accounts, expected, at, evidence)?;
        let mut updates = take_retained(&mut self.accounts, evidence)?;
        let after_nodes = account_nodes(&self.model);
        updates.updated_nodes.retain(|path, node| before_nodes.get(path) != Some(node));
        updates.removed_nodes.retain(|path| before_nodes.contains_key(path));
        let expected_updates = SparseTrieUpdates {
            updated_nodes: after_nodes
                .iter()
                .filter(|(path, node)| before_nodes.get(path) != Some(*node))
                .map(|(path, node)| (*path, node.clone()))
                .collect(),
            removed_nodes: before_nodes
                .keys()
                .filter(|path| !after_nodes.contains_key(path))
                .copied()
                .collect(),
        };
        equal(updates.clone(), expected_updates, "account-retained-updates-oracle")?;
        evidence.checkpoints.push((expected, updates));
        evidence.transcript.push(format!("account-change={change:?};state-root={expected:?}"));
        Ok(())
    }

    fn actions(
        &mut self,
        actions: &[Action],
        at: u64,
        clustered: bool,
        factory: &impl Fn() -> T,
        evidence: &mut Evidence,
    ) -> Result<(), DifferentialFailure> {
        for action in actions {
            match action {
                Action::UpdateBatch { updates } => {
                    for change in updates {
                        let key = B256::from(change.key());
                        if matches!(change, Change::DeleteAccount { .. }) &&
                            self.model.contains_key(&key)
                        {
                            evidence.features.insert("generated_account_deleted");
                            if clustered {
                                evidence.hit("clustered_account_deletes");
                                evidence.features.insert("generated_clustered_account_deleted");
                            }
                            if !self.model[&key].storage.is_empty() {
                                evidence.features.insert("generated_account_storage_wiped");
                            }
                        }
                        if matches!(change, Change::Account { .. }) &&
                            !self.model.contains_key(&key) &&
                            self.deleted.contains(&key)
                        {
                            evidence.features.insert("generated_account_recreated");
                        }
                        self.change(change, at, factory, evidence)?;
                    }
                }
                Action::Prewarm { key, slot } => {
                    let key = B256::from(*key);
                    if let Some(account) = self.model.get(&key) {
                        if let Some(slot) = slot {
                            let harness = TrieTestHarness::new(account.storage.clone());
                            let (mut nodes, _) = harness.proof_v2(&mut [ProofV2Target::new(*slot)]);
                            reveal_prefix(
                                self.storages.get_mut(&key).expect("created storage trie"),
                                &mut nodes,
                                *slot,
                                evidence,
                            )?;
                        } else {
                            equal(
                                self.accounts.get_leaf_value(&Nibbles::unpack(key)).cloned(),
                                Some(encoded(account, oracle(&account.storage))),
                                "account-prewarm-lookup",
                            )?;
                        }
                    }
                }
                Action::Prune { cutoff } => {
                    for (key, storage) in &mut self.storages {
                        if let Some(account) = self.model.get(key) {
                            prune(storage, *cutoff, evidence);
                            root(storage, oracle(&account.storage), at, evidence)?;
                        }
                    }
                }
                Action::Clone | Action::Snapshot => {
                    let mut fork = if matches!(action, Action::Snapshot) {
                        self.accounts.snapshot()
                    } else {
                        self.accounts.clone()
                    };
                    root(&mut fork, state_root(&self.model), at, evidence)?;
                }
                Action::TrackingReset => {
                    self.accounts.set_updates(true);
                    for storage in self.storages.values_mut() {
                        storage.set_updates(true);
                    }
                }
                Action::Reveal { .. } => {}
            }
        }
        Ok(())
    }
}

pub(super) fn exercise<T: SparseTrie + Clone>(
    input: &Input,
    factory: impl Fn() -> T,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    let mut state = State::new(&factory, evidence)?;
    for (round, actions) in input.rounds.iter().enumerate() {
        state.actions(
            actions,
            1000 + round as u64,
            input.clustered.as_ref().is_some_and(|range| range.contains(&round)),
            &factory,
            evidence,
        )?;
    }
    // Every execution also checks a populated wipe, same-batch recreation and stale-slot absence.
    let key = [0x91; 32];
    let mut state = State::new(&factory, evidence)?;
    let changes = [
        Change::Account { key, nonce: 1, value: U256::ONE.to_be_bytes() },
        Change::Storage { key, slot: [0x31; 32], value: U256::from(7).to_be_bytes() },
        Change::Storage { key, slot: [0x32; 32], value: U256::from(9).to_be_bytes() },
        Change::DeleteAccount { key },
        Change::Account { key, nonce: 2, value: U256::from(2).to_be_bytes() },
        Change::Storage { key, slot: [0x32; 32], value: U256::from(11).to_be_bytes() },
        Change::Storage { key, slot: [0x32; 32], value: [0; 32] },
        Change::DeleteAccount { key },
    ];
    for (index, change) in changes.iter().enumerate() {
        state.change(change, 1100 + index as u64, &factory, evidence)?;
    }
    equal(state.model.is_empty(), true, "account-delete-last")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn populated_wipe_and_recreation_do_not_restore_old_slots() {
        let factory = ArenaParallelSparseTrie::default;
        let mut evidence = Evidence::default();
        let mut state = State::new(&factory, &mut evidence).unwrap();
        let key = [0x12; 32];
        for (index, change) in [
            Change::Account { key, nonce: 1, value: U256::ONE.to_be_bytes() },
            Change::Storage { key, slot: [0x33; 32], value: U256::from(7).to_be_bytes() },
            Change::Storage { key, slot: [0x34; 32], value: U256::from(9).to_be_bytes() },
            Change::DeleteAccount { key },
            Change::Account { key, nonce: 1, value: U256::ONE.to_be_bytes() },
        ]
        .iter()
        .enumerate()
        {
            state.change(change, index as u64 + 1, &factory, &mut evidence).unwrap();
        }
        let account = &state.model[&B256::from(key)];
        assert!(account.storage.is_empty() && account.touched.is_empty());
        let storage = state.storages.get_mut(&B256::from(key)).unwrap();
        assert_eq!(storage.root(epoch(10)), EMPTY_ROOT_HASH);
        assert_eq!(storage.get_leaf_value(&Nibbles::unpack([0x33; 32])), None);
        assert_eq!(storage.get_leaf_value(&Nibbles::unpack([0x34; 32])), None);
        assert_eq!(evidence.operations["account_storage_wipes"], 1);
        assert_eq!(evidence.operations["account_recreates"], 1);
    }

    #[test]
    fn mutated_absent_storage_is_noop_and_absent_delete_is_not_counted() {
        let factory = ArenaParallelSparseTrie::default;
        let mut evidence = Evidence::default();
        let mut state = State::new(&factory, &mut evidence).unwrap();
        state
            .actions(
                &[Action::UpdateBatch {
                    updates: vec![
                        Change::Storage {
                            key: [0x11; 32],
                            slot: [0x22; 32],
                            value: U256::ONE.to_be_bytes(),
                        },
                        Change::DeleteAccount { key: [0x11; 32] },
                    ],
                }],
                1,
                true,
                &factory,
                &mut evidence,
            )
            .unwrap();
        assert!(state.model.is_empty());
        assert!(!evidence.operations.contains_key("account_deletes"));
        assert!(!evidence.operations.contains_key("clustered_account_deletes"));
    }
}
