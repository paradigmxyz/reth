//! Deterministic, single-threaded, in-memory differential sparse-trie workload.
//!
//! Run with `cargo run -p reth-trie-sparse --example sparse_trie_differential -- 0 1 2`.
//! No global pool, database, filesystem, clock, or OS randomness is used by `run`.

use alloy_primitives::{map::B256Map, B256, U256};
use alloy_rlp::encode_fixed_size;
use alloy_trie::EMPTY_ROOT_HASH;
use reth_trie::test_utils::{storage_root_prehashed, TrieTestHarness};
use reth_trie_common::{ExtensionNode, Nibbles, ProofV2Target, RlpNode, TrieNodeV2};
use reth_trie_sparse::{
    errors::SparseTrieErrorKind, ArenaParallelSparseTrie, ArenaParallelismThresholds, LeafLookup,
    LeafLookupError, LeafUpdate, PersistentArenaParallelSparseTrie,
    PersistentArenaParallelismThresholds, SparseTrie, SparseTrieUpdates, TrieNodeEpoch,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
};

/// Stable ordered dimensions for measured, instrumented workload coverage.
pub const COVERAGE_OPERATIONS: &[&str] = &[
    "blocked_update_batches",
    "clear",
    "clone",
    "delete",
    "find_leaf",
    "get_leaf_value",
    "insert",
    "is_root_cached",
    "modify",
    "proof_batches",
    "prune",
    "reserve",
    "reveal_nodes",
    "root",
    "root_epoch",
    "scheduled_actions",
    "set_root",
    "set_updates",
    "snapshot",
    "take_updates",
    "update_leaves",
    "update_subtrie_hashes",
    "updates_ref",
    "with_capacity",
];

/// An intentional objective mismatch, distinct from an unexpected library/harness panic.
#[derive(Debug)]
pub struct DifferentialFailure {
    /// Stable category, independent of randomized data and backend debug formatting.
    pub signature: String,
    /// Reproduction context and the unequal observations.
    pub details: String,
}

#[derive(Default)]
struct Evidence {
    operations: BTreeMap<&'static str, u64>,
    features: BTreeSet<&'static str>,
    checkpoints: Vec<(B256, SparseTrieUpdates)>,
    transcript: Vec<String>,
}

impl Evidence {
    fn hit(&mut self, operation: &'static str) {
        assert!(COVERAGE_OPERATIONS.contains(&operation), "unregistered coverage operation");
        *self.operations.entry(operation).or_default() += 1;
    }
}

/// Version of the explicit, locally mutable workload input.
pub const INPUT_VERSION: u8 = 2;

/// A generated fixture plus an explicit action sequence for each mandatory round.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Serialized shape version; older seed-only inputs are not accepted.
    pub version: u8,
    /// Seed for the fixed trait-check spine, never used to regenerate explicit actions.
    pub fixture_seed: u64,
    /// Exactly sixteen rounds; an empty action list leaves that round's fixed checks intact.
    pub rounds: Vec<Vec<Action>>,
}

/// A locally mutable action, executed in order after a round's mandatory checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Apply a batch (empty is a no-op); repeated keys use the last value in this batch.
    UpdateBatch {
        /// At most four key/value changes, with zero values representing deletion.
        updates: Vec<Change>,
    },
    /// Reveal a valid model-derived proof, including for an absent key.
    Reveal {
        /// Prehashed key, not a database lookup or a proof supplied by the fuzzer.
        key: [u8; 32],
    },
    /// Blind nodes older than the supplied epoch.
    Prune {
        /// Absolute epoch cutoff; zero retains all nodes.
        cutoff: u64,
    },
    /// Snapshot and verify the current root without modifying the source.
    Snapshot,
    /// Clone and verify the current root without modifying the source.
    Clone,
    /// Reset retained-update tracking and verify its accumulator is empty.
    TrackingReset,
}

/// An explicit storage-leaf change; arrays keep serialization independent of alloy features.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// Already hashed, full-width trie key.
    pub key: [u8; 32],
    /// Big-endian integer; zero deletes, nonzero is canonically RLP encoded by the driver.
    pub value: [u8; 32],
}

impl Input {
    /// Generates the explicit tape once; execution consumes it without drawing actions again.
    pub fn generate(seed: u64) -> Self {
        let mut generator = Generator(seed ^ 0x494e_5055_5400_0002);
        let rounds = (0..16)
            .map(|round| {
                let mut actions = vec![Action::UpdateBatch {
                    updates: (0..=round % 2)
                        .map(|_| Change {
                            key: generator.key().0,
                            value: generator.value().to_be_bytes(),
                        })
                        .collect(),
                }];
                actions.push(match generator.next() % 5 {
                    0 => Action::Prune { cutoff: if round % 2 == 0 { 0 } else { 18 + round * 10 } },
                    1 => Action::Reveal { key: generator.key().0 },
                    2 => Action::Snapshot,
                    3 => Action::Clone,
                    _ => Action::TrackingReset,
                });
                actions
            })
            .collect();
        Self { version: INPUT_VERSION, fixture_seed: seed, rounds }
    }

    /// Checks shape bounds before executing either backend; no target work occurs on failure.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != INPUT_VERSION {
            return Err("unsupported input version");
        }
        if self.rounds.len() != 16 {
            return Err("input requires sixteen rounds");
        }
        for actions in &self.rounds {
            if actions.len() > 8 {
                return Err("round exceeds eight actions");
            }
            for action in actions {
                if let Action::UpdateBatch { updates } = action &&
                    updates.len() > 4
                {
                    return Err("batch exceeds four updates");
                }
            }
        }
        Ok(())
    }
}

// A fixed algorithm keeps seeds reproducible across rand versions and native adapters.
struct Generator(u64);

impl Generator {
    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    fn key(&mut self) -> B256 {
        let mut bytes = [0; 32];
        for part in bytes.as_chunks_mut::<8>().0 {
            part.copy_from_slice(&self.next().to_be_bytes());
        }
        B256::from(bytes)
    }

    fn value(&mut self) -> U256 {
        match self.next() % 4 {
            0 => U256::ONE,
            1 => U256::from(self.next() % 127 + 1),
            2 => U256::from(self.next() | 1),
            _ => U256::from_be_bytes(self.key().0) | U256::ONE,
        }
    }
}

fn equal<T: Debug + PartialEq>(
    actual: T,
    expected: T,
    signature: &str,
) -> Result<(), DifferentialFailure> {
    if actual != expected {
        return Err(DifferentialFailure {
            signature: format!("sparse-trie/{signature}"),
            details: format!("actual={actual:?}; expected={expected:?}"),
        });
    }
    Ok(())
}

fn succeeded<T, E: Debug>(result: Result<T, E>, operation: &str) -> Result<T, DifferentialFailure> {
    result.map_err(|error| DifferentialFailure {
        signature: format!("sparse-trie/{operation}"),
        details: format!("unexpected error for valid input: {error:?}"),
    })
}

const fn epoch(value: u64) -> TrieNodeEpoch {
    TrieNodeEpoch::new(value)
}

fn oracle(model: &BTreeMap<B256, U256>) -> B256 {
    storage_root_prehashed(model.iter().map(|(&key, &value)| (key, value)))
}

fn leaf_updates(changes: &BTreeMap<B256, U256>) -> B256Map<LeafUpdate> {
    changes
        .iter()
        .map(|(&key, &value)| {
            (
                key,
                LeafUpdate::Changed(if value.is_zero() {
                    Vec::new()
                } else {
                    encode_fixed_size(&value).to_vec()
                }),
            )
        })
        .collect()
}

fn initialize<T: SparseTrie>(
    trie: &mut T,
    harness: &TrieTestHarness,
    retain: bool,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    let root = harness.root_node();
    succeeded(trie.set_root(root.node, root.masks, retain), "set-root")?;
    evidence.hit("set_root");
    equal(harness.original_root(), oracle(harness.storage()), "oracle-agreement")?;
    Ok(())
}

fn reveal<T: SparseTrie>(
    trie: &mut T,
    harness: &TrieTestHarness,
    targets: &mut [ProofV2Target],
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    targets.sort_by_key(|target| target.key_nibbles);
    let (mut nodes, _) = harness.proof_v2(targets);
    // Reverse depth order deliberately: the public API accepts unsorted proofs.
    nodes.sort_by_key(|node| std::cmp::Reverse(node.path));
    for node in &nodes {
        if node.path.len() >= 4 {
            evidence.features.insert("proof_below_subtrie_boundary");
        }
        if node.path.len() >= 60 {
            evidence.features.insert("proof_deep_60_nibbles");
        }
    }
    succeeded(trie.reveal_nodes(&mut nodes), "reveal-nodes")?;
    evidence.hit("reveal_nodes");
    evidence.hit("proof_batches");
    Ok(())
}

fn apply<T: SparseTrie>(
    trie: &mut T,
    harness: &TrieTestHarness,
    pending: &mut B256Map<LeafUpdate>,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    // Bound retries even if a backend makes no progress. A Patricia path has at most 64 nibbles.
    for _ in 0..130 {
        let mut targets = Vec::new();
        succeeded(
            trie.update_leaves(pending, |key, parent| {
                targets.push(ProofV2Target::new(key).with_parent(parent));
            }),
            "update-leaves",
        )?;
        evidence.hit("update_leaves");
        let mut callbacks = targets
            .iter()
            .map(|target| format!("{:?}/{:?}", target.key_nibbles, target.parent))
            .collect::<Vec<_>>();
        callbacks.sort();
        let mut remaining =
            pending.iter().map(|(key, value)| format!("{key:?}/{value:?}")).collect::<Vec<_>>();
        remaining.sort();
        evidence.transcript.push(format!("callbacks={callbacks:?};remaining={remaining:?}"));
        if targets.is_empty() {
            equal(pending.values().any(LeafUpdate::is_changed), false, "pending-changed")?;
            return Ok(());
        }
        evidence.features.insert("blinded_update_retry");
        evidence.hit("blocked_update_batches");
        reveal(trie, harness, &mut targets, evidence)?;
    }
    Err(DifferentialFailure {
        signature: "sparse-trie/retry-bound".into(),
        details: "130 proof retries exhausted".into(),
    })
}

fn root<T: SparseTrie>(
    trie: &mut T,
    expected: B256,
    at: u64,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    equal(trie.root(epoch(at)), expected, "root-oracle")?;
    evidence.hit("root");
    equal(trie.is_root_cached(), true, "root-cache")?;
    evidence.hit("is_root_cached");
    equal(trie.root_epoch().is_some(), true, "clean-root-epoch")?;
    evidence.hit("root_epoch");
    let old_epoch = trie.root_epoch();
    evidence.transcript.push(format!("root-epoch={old_epoch:?};cached={}", trie.is_root_cached()));
    equal(trie.root(epoch(at + 1)), expected, "cached-root-oracle")?;
    evidence.hit("root");
    equal(trie.root_epoch(), old_epoch, "cached-root-epoch-stability")?;
    evidence.hit("root_epoch");
    Ok(())
}

fn lookups<T: SparseTrie>(
    trie: &mut T,
    harness: &TrieTestHarness,
    missing: B256,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    let mut targets =
        harness.storage().keys().map(|key| ProofV2Target::new(*key)).collect::<Vec<_>>();
    targets.push(ProofV2Target::new(missing));
    reveal(trie, harness, &mut targets, evidence)?;
    for (&key, value) in harness.storage() {
        let path = Nibbles::unpack(key);
        let encoded = encode_fixed_size(value).to_vec();
        equal(trie.get_leaf_value(&path), Some(&encoded), "get-leaf-value")?;
        evidence.hit("get_leaf_value");
        equal(trie.find_leaf(&path, Some(&encoded)), Ok(LeafLookup::Exists), "find-exists")?;
        evidence.hit("find_leaf");
        equal(trie.find_leaf(&path, None), Ok(LeafLookup::Exists), "find-exists-no-value")?;
        evidence.hit("find_leaf");
        let wrong = vec![0xff; encoded.len() + 1];
        equal(
            trie.find_leaf(&path, Some(&wrong)),
            Err(LeafLookupError::ValueMismatch { path, expected: Some(wrong), actual: encoded }),
            "find-value-mismatch",
        )?;
        evidence.hit("find_leaf");
    }
    equal(trie.get_leaf_value(&Nibbles::unpack(missing)), None, "missing-value")?;
    evidence.hit("get_leaf_value");
    equal(
        trie.find_leaf(&Nibbles::unpack(missing), None),
        Ok(LeafLookup::NonExistent),
        "find-absent",
    )?;
    evidence.hit("find_leaf");
    evidence.features.extend(["lookup_exists", "lookup_absent", "lookup_mismatch"]);
    Ok(())
}

fn verify_snapshot_values<T: SparseTrie>(
    trie: &T,
    model: &BTreeMap<B256, U256>,
    candidates: &BTreeMap<B256, U256>,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    // Historical snapshots are taken fully revealed. Do not repair them with fresh proofs.
    for (&key, value) in model {
        let path = Nibbles::unpack(key);
        let encoded = encode_fixed_size(value).to_vec();
        equal(trie.get_leaf_value(&path), Some(&encoded), "snapshot-leaf-isolation")?;
        evidence.hit("get_leaf_value");
        equal(
            trie.find_leaf(&path, Some(&encoded)),
            Ok(LeafLookup::Exists),
            "snapshot-find-isolation",
        )?;
        evidence.hit("find_leaf");
    }
    for key in candidates.keys().filter(|key| !model.contains_key(*key)) {
        let path = Nibbles::unpack(*key);
        equal(trie.get_leaf_value(&path), None, "snapshot-no-leaked-insert")?;
        evidence.hit("get_leaf_value");
        equal(
            trie.find_leaf(&path, None),
            Ok(LeafLookup::NonExistent),
            "snapshot-excludes-future-key",
        )?;
        evidence.hit("find_leaf");
    }
    Ok(())
}

fn retained<T: SparseTrie>(
    trie: &mut T,
    harness: &TrieTestHarness,
    changes: &BTreeMap<B256, U256>,
    evidence: &mut Evidence,
) -> Result<SparseTrieUpdates, DifferentialFailure> {
    let before = trie.updates_ref().into_owned();
    evidence.hit("updates_ref");
    let mut updates = trie.take_updates();
    evidence.hit("take_updates");
    equal(updates.clone(), before, "updates-ref-take")?;
    equal(trie.take_updates(), SparseTrieUpdates::default(), "take-drains")?;
    evidence.hit("take_updates");
    equal(
        updates.removed_nodes.iter().any(|path| updates.updated_nodes.contains_key(path)),
        false,
        "update-disjointness",
    )?;
    updates
        .updated_nodes
        .retain(|path, node| harness.storage_trie_updates().storage_nodes.get(path) != Some(node));
    updates
        .removed_nodes
        .retain(|path| harness.storage_trie_updates().storage_nodes.contains_key(path));
    let (_, mut expected) = harness.get_root_with_updates(changes);
    harness.minimize_trie_updates(&mut expected);
    let normalized = SparseTrieUpdates {
        updated_nodes: expected.storage_nodes.into_iter().collect(),
        removed_nodes: expected.removed_nodes.into_iter().collect(),
    };
    equal(updates.clone(), normalized, "retained-updates-oracle")?;
    if !updates.updated_nodes.is_empty() {
        evidence.features.insert("retained_updated_branches");
    }
    if !updates.removed_nodes.is_empty() {
        evidence.features.insert("retained_removed_branches");
    }
    Ok(updates)
}

fn prune<T: SparseTrie>(trie: &mut T, cutoff: u64, evidence: &mut Evidence) -> usize {
    let count = trie.prune(epoch(cutoff));
    evidence.hit("prune");
    evidence.transcript.push(format!(
        "prune-cutoff={cutoff};count={count};epoch={:?};cached={}",
        trie.root_epoch(),
        trie.is_root_cached()
    ));
    count
}

fn randomized_actions<T: SparseTrie + Clone>(
    trie: &mut T,
    harness: &mut TrieTestHarness,
    actions: &[Action],
    at: u64,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    for action in actions {
        evidence.hit("scheduled_actions");
        evidence.transcript.push(format!("scheduled-action={action:?}"));
        match action {
            Action::Prune { cutoff } => {
                prune(trie, *cutoff, evidence);
                evidence.features.insert(if *cutoff == 0 {
                    "schedule_prune_zero"
                } else {
                    "schedule_prune_recent"
                });
            }
            Action::Reveal { key } => {
                reveal(trie, harness, &mut [ProofV2Target::new(B256::from(*key))], evidence)?;
                evidence.features.insert("schedule_reveal");
            }
            Action::Snapshot | Action::Clone => {
                let snapshot = matches!(action, Action::Snapshot);
                let mut fork = if snapshot {
                    evidence.hit("snapshot");
                    trie.snapshot()
                } else {
                    evidence.hit("clone");
                    trie.clone()
                };
                root(&mut fork, harness.original_root(), at, evidence)?;
                drop(fork);
                evidence.features.insert(if snapshot {
                    "schedule_snapshot"
                } else {
                    "schedule_clone"
                });
            }
            Action::TrackingReset => {
                trie.set_updates(true);
                evidence.hit("set_updates");
                equal(
                    trie.updates_ref().as_ref(),
                    &SparseTrieUpdates::default(),
                    "schedule-tracking-reset",
                )?;
                evidence.features.insert("schedule_tracking_reset");
            }
            Action::UpdateBatch { updates } => {
                let changes = updates
                    .iter()
                    .map(|change| (B256::from(change.key), U256::from_be_bytes(change.value)))
                    .collect::<BTreeMap<_, _>>();
                for (key, value) in &changes {
                    evidence.hit(if value.is_zero() {
                        "delete"
                    } else if harness.storage().contains_key(key) {
                        "modify"
                    } else {
                        "insert"
                    });
                }
                apply(trie, harness, &mut leaf_updates(&changes), evidence)?;
                let expected = oracle(&model_after(harness.storage(), &changes));
                root(trie, expected, at, evidence)?;
                let updates = retained(trie, harness, &changes, evidence)?;
                evidence.checkpoints.push((expected, updates));
                harness.apply_changeset(changes);
                evidence.features.insert("schedule_mutation");
            }
        }
        root(trie, harness.original_root(), at, evidence)?;
    }
    Ok(())
}

fn model_after(
    model: &BTreeMap<B256, U256>,
    changes: &BTreeMap<B256, U256>,
) -> BTreeMap<B256, U256> {
    let mut model = model.clone();
    for (&key, &value) in changes {
        if value.is_zero() {
            model.remove(&key);
        } else {
            model.insert(key, value);
        }
    }
    model
}

fn generated(seed: u64) -> BTreeMap<B256, U256> {
    let mut generator = Generator(seed);
    let mut model = BTreeMap::new();
    for _ in 0..48 {
        model.insert(generator.key(), generator.value());
    }
    // Dense real branch nodes on both sides of the arena's four-nibble boundary.
    for group in [0x10, 0x20] {
        for branch in 0..32u8 {
            for child in [0, 0x10] {
                let mut key = generator.key();
                key.0[0] = group;
                key.0[1] = branch;
                key.0[2] = child;
                model.insert(key, generator.value());
            }
        }
    }
    // Embedded short leaves, long extensions, and branches at nibble 63.
    for suffix in 0..16u8 {
        let mut key = B256::repeat_byte(0xdd);
        key.0[31] = 0xf0 | suffix;
        model.insert(key, U256::from(u64::from(suffix) + 1));
    }
    model
}

fn exercise<T: SparseTrie + Clone>(
    input: &Input,
    factory: fn() -> T,
) -> Result<Evidence, DifferentialFailure> {
    let seed = input.fixture_seed;
    let mut evidence = Evidence::default();
    let mut generator = Generator(seed ^ 0xa0761d6478bd642f);
    let mut harness = TrieTestHarness::new(generated(seed));
    root_and_fork_isolation(factory, &harness, &mut evidence)?;
    let mut trie = factory();
    initialize(&mut trie, &harness, true, &mut evidence)?;
    succeeded(trie.reveal_nodes(&mut []), "empty-reveal")?;
    evidence.hit("reveal_nodes");
    root(&mut trie, harness.original_root(), 1, &mut evidence)?;
    // Root-only materialization must leave at least one non-embedded leaf blinded.
    let blind_key = harness
        .storage()
        .keys()
        .find(|key| {
            matches!(
                trie.find_leaf(&Nibbles::unpack(**key), None),
                Err(LeafLookupError::BlindedNode { .. })
            )
        })
        .copied();
    equal(blind_key.is_some(), true, "root-only-blinded")?;
    if let Some(key) = blind_key {
        let path = Nibbles::unpack(key);
        equal(trie.get_leaf_value(&path), None, "blind-get-value")?;
        evidence.hit("get_leaf_value");
        match trie.find_leaf(&path, None) {
            Err(LeafLookupError::BlindedNode { path: blinded, hash }) => {
                equal(path.starts_with(&blinded), true, "blind-error-prefix")?;
                equal(hash == B256::ZERO, false, "blind-error-hash")?;
            }
            actual => {
                return Err(DifferentialFailure {
                    signature: "sparse-trie/find-blind".into(),
                    details: format!("{actual:?}"),
                })
            }
        }
        evidence.hit("find_leaf");
        evidence.features.insert("lookup_blinded");
        let mut touched = std::iter::once((key, LeafUpdate::Touched)).collect();
        apply(&mut trie, &harness, &mut touched, &mut evidence)?;
        root(&mut trie, harness.original_root(), 2, &mut evidence)?;
        evidence.features.insert("touched_blinded");
    }
    // Progressively reveal valid ancestor-closed proof subsets at different depths.
    let mut targets =
        harness.storage().keys().map(|key| ProofV2Target::new(*key)).collect::<Vec<_>>();
    let (all_nodes, _) = harness.proof_v2(&mut targets);
    for depth in [0, 1, 3, 4, 8, 32, 63, 64] {
        let mut nodes =
            all_nodes.iter().filter(|node| node.path.len() <= depth).cloned().collect::<Vec<_>>();
        nodes.reverse();
        succeeded(trie.reveal_nodes(&mut nodes), "depth-reveal")?;
        evidence.hit("reveal_nodes");
        root(&mut trie, harness.original_root(), 3, &mut evidence)?;
    }
    evidence.features.insert("progressive_revelation_depth");
    equal(trie.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED), "proof-unmodified-epoch")?;
    let proof_pruned = prune(&mut trie, 1, &mut evidence);
    equal(proof_pruned > 0, true, "proof-unmodified-prune")?;
    root(&mut trie, harness.original_root(), 4, &mut evidence)?;
    evidence.features.insert("prune_unmodified_proof");
    // Restore root-only state so the randomized mutation phase still exercises proof retries.
    trie.clear();
    evidence.hit("clear");
    initialize(&mut trie, &harness, true, &mut evidence)?;
    let mut history = Vec::new();
    for round in 0..16u64 {
        let at = 10 + round * 10;
        let mut missing = generator.key();
        while harness.storage().contains_key(&missing) {
            missing = generator.key();
        }
        let mut changes = BTreeMap::new();
        let keys = harness.storage().keys().copied().collect::<Vec<_>>();
        for _ in 0..if keys.is_empty() { 0 } else { 12 } {
            let key = keys[(generator.next() % keys.len() as u64) as usize];
            let value =
                if generator.next().is_multiple_of(3) { U256::ZERO } else { generator.value() };
            changes.insert(key, value);
        }
        for _ in 0..8 {
            changes.insert(generator.key(), generator.value());
        }
        if round == 3 {
            for key in keys.iter().filter(|key| key.0[0] == 0x20) {
                changes.insert(*key, U256::ZERO);
            }
            evidence.features.insert("delete_whole_subtrie");
        }
        for (&key, &value) in &changes {
            evidence.hit(if value.is_zero() {
                "delete"
            } else if harness.storage().contains_key(&key) {
                "modify"
            } else {
                "insert"
            });
        }
        let model = model_after(harness.storage(), &changes);
        let mut pending = leaf_updates(&changes);
        pending.reserve(16);
        evidence.hit("reserve");
        pending.insert(missing, LeafUpdate::Touched);
        let mut absent_delete = generator.key();
        while harness.storage().contains_key(&absent_delete) || changes.contains_key(&absent_delete)
        {
            absent_delete = generator.key();
        }
        pending.insert(absent_delete, LeafUpdate::Changed(Vec::new()));
        evidence.features.insert("delete_absent");
        apply(&mut trie, &harness, &mut pending, &mut evidence)?;
        evidence.features.insert("touched_absent");
        // Fork while dirty, before hashing and before draining accumulated updates.
        let dirty_epoch = trie.root_epoch();
        let dirty_cached = trie.is_root_cached();
        let mut dirty_snapshot = trie.snapshot();
        evidence.hit("snapshot");
        equal(dirty_epoch, None, "dirty-root-epoch")?;
        equal(dirty_cached, false, "dirty-root-cache")?;
        equal(dirty_snapshot.root_epoch(), dirty_epoch, "snapshot-dirty-epoch")?;
        equal(dirty_snapshot.is_root_cached(), dirty_cached, "snapshot-dirty-cache")?;
        equal(dirty_snapshot.updates_ref(), trie.updates_ref(), "snapshot-pending-updates")?;
        let mut dirty_clone = trie.clone();
        evidence.hit("clone");
        equal(dirty_clone.root_epoch(), dirty_epoch, "clone-dirty-epoch")?;
        equal(dirty_clone.is_root_cached(), dirty_cached, "clone-dirty-cache")?;
        equal(dirty_clone.updates_ref(), trie.updates_ref(), "clone-pending-updates")?;
        trie.update_subtrie_hashes(epoch(at));
        evidence.hit("update_subtrie_hashes");
        root(&mut trie, oracle(&model), at, &mut evidence)?;
        root(&mut dirty_snapshot, oracle(&model), at, &mut evidence)?;
        root(&mut dirty_clone, oracle(&model), at, &mut evidence)?;
        equal(dirty_snapshot.root_epoch(), trie.root_epoch(), "snapshot-hashed-epoch")?;
        let updates_before_take = trie.updates_ref().into_owned();
        equal(
            dirty_snapshot.updates_ref().as_ref(),
            &updates_before_take,
            "snapshot-hashed-updates",
        )?;
        let updates = retained(&mut trie, &harness, &changes, &mut evidence)?;
        equal(
            dirty_snapshot.updates_ref().as_ref(),
            &updates_before_take,
            "source-drain-isolation",
        )?;
        equal(dirty_clone.take_updates(), updates_before_take.clone(), "clone-updates-isolation")?;
        equal(dirty_snapshot.take_updates(), updates_before_take, "snapshot-drain-isolation")?;
        evidence.hit("take_updates");
        evidence.hit("take_updates");
        evidence.checkpoints.push((oracle(&model), updates));
        // Both dirty-created copies must accept independent leaf changes after hashing.
        let fork_harness = TrieTestHarness::new(model.clone());
        for independent in [&mut dirty_snapshot, &mut dirty_clone] {
            let fork_changes = BTreeMap::from([(missing, generator.value())]);
            apply(independent, &fork_harness, &mut leaf_updates(&fork_changes), &mut evidence)?;
            root(independent, oracle(&model_after(&model, &fork_changes)), at + 1, &mut evidence)?;
            root(&mut trie, oracle(&model), at + 1, &mut evidence)?;
            equal(
                trie.get_leaf_value(&Nibbles::unpack(missing)),
                None,
                "dirty-fork-leaf-isolation",
            )?;
            evidence.hit("get_leaf_value");
        }
        harness = TrieTestHarness::new(model);
        // Exercise proof idempotence and every leaf against the independently maintained model.
        lookups(&mut trie, &harness, missing, &mut evidence)?;
        lookups(&mut trie, &harness, missing, &mut evidence)?;
        if round % 4 == 0 {
            history.push((trie.snapshot(), harness.storage().clone(), trie.root_epoch()));
            history.push((trie.clone(), harness.storage().clone(), trie.root_epoch()));
            evidence.hit("snapshot");
            evidence.hit("clone");
        }
        // Mutate a fork in the opposite direction, including prune and clear on that fork.
        let mut fork = trie.snapshot();
        evidence.hit("snapshot");
        let fork_changes = BTreeMap::from([(missing, generator.value())]);
        apply(&mut fork, &harness, &mut leaf_updates(&fork_changes), &mut evidence)?;
        root(
            &mut fork,
            oracle(&model_after(harness.storage(), &fork_changes)),
            at + 2,
            &mut evidence,
        )?;
        prune(&mut fork, at + 3, &mut evidence);
        fork.clear();
        evidence.hit("clear");
        root(&mut fork, EMPTY_ROOT_HASH, at + 4, &mut evidence)?;
        drop(fork);
        root(&mut trie, harness.original_root(), at + 4, &mut evidence)?;
        // Equality cutoff retains recent nodes; later cutoff blinds old nodes.
        equal(prune(&mut trie, 0, &mut evidence), 0, "prune-zero-cutoff")?;
        let before_prune_epoch = trie.root_epoch();
        let pruned = prune(&mut trie, if round % 2 == 0 { at } else { at + 5 }, &mut evidence);
        equal(trie.root_epoch(), before_prune_epoch, "prune-epoch-stability")?;
        if pruned > 0 {
            evidence.features.insert("pruned_nodes");
        }
        root(&mut trie, harness.original_root(), at + 6, &mut evidence)?;
        for (old, model, old_epoch) in &mut history {
            verify_snapshot_values(old, model, harness.storage(), &mut evidence)?;
            root(old, oracle(model), at + 7, &mut evidence)?;
            equal(old.root_epoch(), *old_epoch, "historical-epoch")?;
        }
        randomized_actions(
            &mut trie,
            &mut harness,
            &input.rounds[round as usize],
            at + 8,
            &mut evidence,
        )?;
    }
    evidence.features.extend([
        "snapshot_dirty",
        "snapshot_clean",
        "clone_dirty",
        "clone_clean",
        "fork_mutation",
        "historical_roots",
    ]);
    // Toggle tracking on a populated dirty trie while a sibling retains its own accumulator.
    let mut toggle = trie.snapshot();
    evidence.hit("snapshot");
    toggle.set_updates(true);
    evidence.hit("set_updates");
    let key = generator.key();
    let changes = BTreeMap::from([(key, generator.value())]);
    apply(&mut toggle, &harness, &mut leaf_updates(&changes), &mut evidence)?;
    let mut dirty_peer = toggle.snapshot();
    evidence.hit("snapshot");
    toggle.set_updates(false);
    evidence.hit("set_updates");
    let toggle_model = model_after(harness.storage(), &changes);
    root(&mut toggle, oracle(&toggle_model), 180, &mut evidence)?;
    equal(toggle.take_updates(), SparseTrieUpdates::default(), "populated-disable")?;
    evidence.hit("take_updates");
    root(&mut dirty_peer, oracle(&toggle_model), 180, &mut evidence)?;
    retained(&mut dirty_peer, &harness, &changes, &mut evidence)?;
    toggle.set_updates(true);
    evidence.hit("set_updates");
    let toggle_harness = TrieTestHarness::new(toggle_model);
    let changes = BTreeMap::from([(key, generator.value())]);
    apply(&mut toggle, &toggle_harness, &mut leaf_updates(&changes), &mut evidence)?;
    // Enabling tracking again resets old accumulators, even while the trie is dirty.
    toggle.set_updates(true);
    evidence.hit("set_updates");
    let newer_model = model_after(toggle_harness.storage(), &changes);
    root(&mut toggle, oracle(&newer_model), 181, &mut evidence)?;
    let retained_before_reset = toggle.updates_ref().into_owned();
    let mut clean_peer = toggle.snapshot();
    evidence.hit("snapshot");
    toggle.set_updates(true);
    evidence.hit("set_updates");
    equal(toggle.take_updates(), SparseTrieUpdates::default(), "populated-enable-reset")?;
    evidence.hit("take_updates");
    equal(clean_peer.take_updates(), retained_before_reset, "tracking-toggle-sibling-isolation")?;
    evidence.hit("take_updates");
    root(&mut trie, harness.original_root(), 182, &mut evidence)?;
    evidence.features.insert("populated_tracking_toggle_isolation");
    trie.clear();
    evidence.hit("clear");
    root(&mut trie, EMPTY_ROOT_HASH, 200, &mut evidence)?;
    // clear preserves the tracking setting; set_updates covers disabling and re-enabling it.
    for retain in [true, false, true] {
        trie.set_updates(retain);
        evidence.hit("set_updates");
        let empty = TrieTestHarness::new(BTreeMap::new());
        let changes = generated(generator.next());
        apply(&mut trie, &empty, &mut leaf_updates(&changes), &mut evidence)?;
        root(&mut trie, oracle(&changes), 210, &mut evidence)?;
        if retain {
            retained(&mut trie, &empty, &changes, &mut evidence)?;
        } else {
            equal(trie.take_updates(), SparseTrieUpdates::default(), "tracking-disabled")?;
            evidence.hit("take_updates");
        }
        trie.clear();
        evidence.hit("clear");
    }
    // Leaf and long-extension roots, delete-last-leaf, empty changes, and reuse after clear.
    for count in [0u8, 1, 2, 16] {
        let model = (0..count)
            .map(|suffix| (B256::with_last_byte(suffix), U256::from(u64::from(suffix) + 1)))
            .collect();
        let small = TrieTestHarness::new(model);
        initialize(&mut trie, &small, count % 2 != 0, &mut evidence)?;
        let missing = B256::repeat_byte(0xff);
        lookups(&mut trie, &small, missing, &mut evidence)?;
        root(&mut trie, small.original_root(), 220, &mut evidence)?;
        let deletes = small.storage().keys().map(|&key| (key, U256::ZERO)).collect();
        apply(&mut trie, &small, &mut leaf_updates(&deletes), &mut evidence)?;
        root(&mut trie, EMPTY_ROOT_HASH, 221, &mut evidence)?;
        trie.clear();
        evidence.hit("clear");
    }
    // The trait exposes no arena reserve/capacity API; exercise the public update container.
    let mut capacity = SparseTrieUpdates::with_capacity(32, 16);
    equal(capacity.updated_nodes.capacity() >= 32, true, "updated-capacity")?;
    equal(capacity.removed_nodes.capacity() >= 16, true, "removed-capacity")?;
    capacity.updated_nodes.reserve(32);
    capacity.removed_nodes.reserve(16);
    let sample = evidence.checkpoints[0].1.clone();
    capacity.extend(sample.clone());
    equal(capacity.clone(), sample, "updates-extend")?;
    let allocated = (capacity.updated_nodes.capacity(), capacity.removed_nodes.capacity());
    capacity.clear();
    equal(
        (capacity.updated_nodes.capacity(), capacity.removed_nodes.capacity()),
        allocated,
        "updates-clear-preserves-capacity",
    )?;
    equal(capacity, SparseTrieUpdates::default(), "updates-clear-extend")?;
    evidence.hit("with_capacity");
    evidence.hit("reserve");
    // Drop the source, then mutate surviving historical snapshots and recheck their siblings.
    drop(trie);
    while let Some((mut survivor, model, _)) = history.pop() {
        verify_snapshot_values(&survivor, &model, harness.storage(), &mut evidence)?;
        root(&mut survivor, oracle(&model), 300, &mut evidence)?;
        let old_harness = TrieTestHarness::new(model.clone());
        let changes = BTreeMap::from([(generator.key(), generator.value())]);
        apply(&mut survivor, &old_harness, &mut leaf_updates(&changes), &mut evidence)?;
        root(&mut survivor, oracle(&model_after(&model, &changes)), 301, &mut evidence)?;
        prune(&mut survivor, 302, &mut evidence);
        survivor.clear();
        evidence.hit("clear");
        for (sibling, sibling_model, _) in &mut history {
            verify_snapshot_values(sibling, sibling_model, &changes, &mut evidence)?;
            root(sibling, oracle(sibling_model), 303, &mut evidence)?;
        }
    }
    evidence.features.extend([
        "source_clear_drop_isolation",
        "snapshot_clear_drop_isolation",
        "tracking_toggle",
        "empty_leaf_extension_branch_roots",
        "delete_last_leaf",
        "capacity_reserve",
    ]);
    Ok(evidence)
}

fn root_and_fork_isolation<T: SparseTrie + Clone>(
    factory: fn() -> T,
    harness: &TrieTestHarness,
    evidence: &mut Evidence,
) -> Result<(), DifferentialFailure> {
    let extension =
        ExtensionNode::new(Nibbles::from_nibbles([1]), RlpNode::word_rlp(&harness.original_root()));
    let mut rejected = factory();
    evidence.hit("set_root");
    match rejected.set_root(TrieNodeV2::Extension(extension.clone()), None, true) {
        Err(error) => match error.into_kind() {
            SparseTrieErrorKind::Reveal { path, node } => {
                equal(path, Nibbles::new(), "extension-error-path")?;
                equal(format!("{node:?}"), format!("{extension:?}"), "extension-error-node")?;
            }
            other => {
                return Err(DifferentialFailure {
                    signature: "sparse-trie/extension-error-kind".into(),
                    details: format!("{other:?}"),
                })
            }
        },
        Ok(()) => {
            return Err(DifferentialFailure {
                signature: "sparse-trie/extension-accepted".into(),
                details: "standalone Extension root must be rejected".into(),
            })
        }
    }
    root(&mut rejected, EMPTY_ROOT_HASH, 1, evidence)?;
    initialize(&mut rejected, harness, true, evidence)?;
    root(&mut rejected, harness.original_root(), 2, evidence)?;
    evidence.features.insert("standalone_extension_rejection");

    // These keys are in distinct root branches, so one proof cannot reveal the other.
    let first = *harness.storage().keys().find(|key| key.0[0] == 0x10).expect("dense first branch");
    let second =
        *harness.storage().keys().find(|key| key.0[0] == 0x20).expect("dense second branch");
    let mut vacant =
        (0..=u8::MAX).map(B256::with_last_byte).filter(|key| !harness.storage().contains_key(key));
    let source_insert = vacant.next().expect("192 base keys leave vacant keys");
    let fork_insert = vacant.next().expect("at least two vacant keys");
    for snapshot in [true, false] {
        let mut source = factory();
        initialize(&mut source, harness, true, evidence)?;
        root(&mut source, harness.original_root(), 1, evidence)?;
        let original_errors =
            [first, second].map(|key| source.find_leaf(&Nibbles::unpack(key), None));
        for original in &original_errors {
            equal(
                matches!(original, Err(LeafLookupError::BlindedNode { .. })),
                true,
                "root-only-fork-blinded",
            )?;
            evidence.transcript.push(format!("root-only-fork-original={original:?}"));
            evidence.hit("find_leaf");
        }
        let mut fork = if snapshot {
            evidence.hit("snapshot");
            source.snapshot()
        } else {
            evidence.hit("clone");
            source.clone()
        };
        for (key, original) in [first, second].into_iter().zip(&original_errors) {
            equal(
                fork.find_leaf(&Nibbles::unpack(key), None),
                original.clone(),
                "fork-original-blind",
            )?;
            evidence.hit("find_leaf");
        }
        // Exercise both directions without repairing the still-blinded sibling.
        for reveal_source in [false, true] {
            let (writer, sibling, key, original) = if reveal_source {
                (&mut source, &mut fork, second, &original_errors[1])
            } else {
                (&mut fork, &mut source, first, &original_errors[0])
            };
            reveal(writer, harness, &mut [ProofV2Target::new(key)], evidence)?;
            let value = encode_fixed_size(&harness.storage()[&key]).to_vec();
            equal(
                writer.get_leaf_value(&Nibbles::unpack(key)),
                Some(&value),
                "proof-writer-value",
            )?;
            equal(
                sibling.find_leaf(&Nibbles::unpack(key), None),
                original.clone(),
                "proof-sibling-isolation",
            )?;
            equal(sibling.get_leaf_value(&Nibbles::unpack(key)), None, "proof-no-leaked-value")?;
            evidence.hit("find_leaf");
            evidence.hit("get_leaf_value");
            evidence.hit("get_leaf_value");
        }
        root(&mut source, harness.original_root(), 2, evidence)?;
        root(&mut fork, harness.original_root(), 2, evidence)?;
        evidence.features.insert(if snapshot {
            "snapshot_blinded_proof_isolation"
        } else {
            "clone_blinded_proof_isolation"
        });

        // Retain a nonempty accumulator, then fork shared dirty state without draining it.
        let priming_value =
            if harness.storage()[&first] == U256::ONE { U256::from(2) } else { U256::ONE };
        let priming = BTreeMap::from([(first, priming_value)]);
        evidence.hit("modify");
        apply(&mut source, harness, &mut leaf_updates(&priming), evidence)?;
        let primed_model = model_after(harness.storage(), &priming);
        root(&mut source, oracle(&primed_model), 3, evidence)?;
        let primed_harness = TrieTestHarness::new(primed_model);
        let mut common = BTreeMap::from([(first, U256::from(100))]);
        evidence.hit("modify");
        // Keep shared deletions dirty while each sibling also owns prior retained updates.
        for key in harness.storage().keys().filter(|key| key.0[0] == 0xdd) {
            common.insert(*key, U256::ZERO);
            evidence.hit("delete");
        }
        apply(&mut source, &primed_harness, &mut leaf_updates(&common), evidence)?;
        equal(source.root_epoch(), None, "shared-dirty-source")?;
        fork = if snapshot {
            evidence.hit("snapshot");
            source.snapshot()
        } else {
            evidence.hit("clone");
            source.clone()
        };
        equal(fork.root_epoch(), None, "shared-dirty-fork")?;
        let fork_pending = fork.updates_ref().into_owned();
        equal(
            fork_pending == SparseTrieUpdates::default(),
            false,
            "shared-dirty-nonempty-updates",
        )?;
        equal(source.updates_ref().as_ref(), &fork_pending, "shared-dirty-pending")?;
        evidence.hit("updates_ref");
        evidence.hit("updates_ref");
        let common_model = model_after(harness.storage(), &common);
        let common_harness = TrieTestHarness::new(common_model);
        let source_changes = BTreeMap::from([
            (first, U256::from(101)),
            (second, U256::ZERO),
            (source_insert, U256::from(11)),
        ]);
        let fork_changes =
            BTreeMap::from([(first, U256::from(102)), (fork_insert, U256::from(22))]);
        evidence.hit("modify");
        evidence.hit("delete");
        evidence.hit("insert");
        evidence.hit("modify");
        evidence.hit("insert");
        apply(&mut source, &common_harness, &mut leaf_updates(&source_changes), evidence)?;
        equal(fork.updates_ref().as_ref(), &fork_pending, "dirty-source-write-pending-isolation")?;
        equal(
            fork.get_leaf_value(&Nibbles::unpack(first)),
            Some(&encode_fixed_size(&U256::from(100)).to_vec()),
            "dirty-source-write-value-isolation",
        )?;
        evidence.hit("updates_ref");
        evidence.hit("get_leaf_value");
        let source_pending = source.updates_ref().into_owned();
        evidence.hit("updates_ref");
        apply(&mut fork, &common_harness, &mut leaf_updates(&fork_changes), evidence)?;
        equal(
            source.updates_ref().as_ref(),
            &source_pending,
            "dirty-fork-write-pending-isolation",
        )?;
        equal(
            source.get_leaf_value(&Nibbles::unpack(first)),
            Some(&encode_fixed_size(&U256::from(101)).to_vec()),
            "dirty-fork-write-value-isolation",
        )?;
        equal(source.root_epoch(), None, "diverged-source-still-dirty")?;
        equal(fork.root_epoch(), None, "diverged-fork-still-dirty")?;
        evidence.hit("updates_ref");
        evidence.hit("get_leaf_value");
        let mut source_total = common.clone();
        source_total.extend(source_changes);
        let mut fork_total = common;
        fork_total.extend(fork_changes);
        let source_model = model_after(harness.storage(), &source_total);
        let fork_model = model_after(harness.storage(), &fork_total);
        let fork_pending = fork.updates_ref().into_owned();
        evidence.hit("updates_ref");
        root(&mut source, oracle(&source_model), 4, evidence)?;
        equal(source.root_epoch(), Some(epoch(4)), "dirty-source-new-epoch")?;
        equal(fork.root_epoch(), None, "source-hash-keeps-fork-dirty")?;
        equal(fork.updates_ref().as_ref(), &fork_pending, "source-hash-pending-isolation")?;
        evidence.hit("updates_ref");
        root(&mut fork, oracle(&fork_model), 4, evidence)?;
        equal(fork.root_epoch(), Some(epoch(4)), "dirty-fork-new-epoch")?;
        let fork_accumulated = fork.updates_ref().into_owned();
        evidence.hit("updates_ref");
        let source_updates = retained(&mut source, harness, &source_total, evidence)?;
        equal(fork.updates_ref().as_ref(), &fork_accumulated, "dirty-source-drain-isolation")?;
        evidence.hit("updates_ref");
        let fork_updates = retained(&mut fork, harness, &fork_total, evidence)?;
        evidence.checkpoints.push((oracle(&source_model), source_updates));
        evidence.checkpoints.push((oracle(&fork_model), fork_updates));
        lookups(&mut source, &TrieTestHarness::new(source_model), fork_insert, evidence)?;
        lookups(&mut fork, &TrieTestHarness::new(fork_model), source_insert, evidence)?;
        evidence.features.insert(if snapshot {
            "snapshot_dirty_prehash_divergence"
        } else {
            "clone_dirty_prehash_divergence"
        });
    }
    Ok(())
}

/// Executes both implementations against independent triehash and in-memory proof/update oracles.
///
/// Returns success JSON with instrumented operation counts and observed features. An intentional
/// mismatch returns `Err`; unexpected backend/harness panics are deliberately not caught.
/// The seed fully determines valid generated inputs. There are no external side effects.
pub fn run(seed: u64) -> Result<String, DifferentialFailure> {
    run_input(&Input::generate(seed))
}

/// Runs the same validated tape on both backends with every fixed trait oracle enabled.
///
/// Invalid shapes fail before target execution. No action generation, I/O or worker spawning
/// occurs.
pub fn run_input(input: &Input) -> Result<String, DifferentialFailure> {
    input.validate().map_err(|details| DifferentialFailure {
        signature: "sparse-trie/invalid-input".into(),
        details: details.into(),
    })?;
    let seed = input.fixture_seed;
    let arena = exercise(input, || {
        ArenaParallelSparseTrie::default().with_parallelism_thresholds(ArenaParallelismThresholds {
            min_dirty_leaves: u64::MAX,
            min_revealed_nodes: usize::MAX,
            min_updates: usize::MAX,
            min_leaves_for_prune: u64::MAX,
        })
    })
    .map_err(|mut failure| {
        failure.details = format!("seed={seed}; backend=arena; {}", failure.details);
        failure
    })?;
    let persistent = exercise(input, || {
        PersistentArenaParallelSparseTrie::default().with_parallelism_thresholds(
            PersistentArenaParallelismThresholds {
                min_dirty_leaves: u64::MAX,
                min_revealed_nodes: usize::MAX,
                min_updates: usize::MAX,
                min_leaves_for_prune: u64::MAX,
            },
        )
    })
    .map_err(|mut failure| {
        failure.details = format!("seed={seed}; backend=persistent; {}", failure.details);
        failure
    })?;
    equal(&arena.checkpoints, &persistent.checkpoints, "backend-checkpoints")?;
    equal(arena.transcript.len(), persistent.transcript.len(), "backend-transcript-length")?;
    for (index, (actual, expected)) in
        arena.transcript.iter().zip(&persistent.transcript).enumerate()
    {
        equal(actual, expected, "backend-observable-transcript").map_err(|mut failure| {
            failure.details = format!("seed={seed}; observation={index}; {}", failure.details);
            failure
        })?;
    }
    let mut counts = arena.operations;
    for (operation, count) in persistent.operations {
        *counts.entry(operation).or_default() += count;
    }
    let operations = counts
        .iter()
        .map(|(name, count)| format!("\"{name}\":{count}"))
        .collect::<Vec<_>>()
        .join(",");
    let features = arena
        .features
        .intersection(&persistent.features)
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("{{\"seed\":{seed},\"operations\":{{{operations}}},\"features\":[{features}],\"backends\":2,\"rounds_per_backend\":16}}"))
}

fn main() {
    let seeds = std::env::args()
        .skip(1)
        .map(|arg| arg.parse::<u64>().expect("seed must be a decimal u64"))
        .collect::<Vec<_>>();
    for seed in if seeds.is_empty() { vec![0] } else { seeds } {
        match run(seed) {
            Ok(report) => println!("{report}"),
            Err(failure) => panic!("{}: {}", failure.signature, failure.details),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproducible_multi_seed() {
        for seed in [0, 1, 2, 42, u64::MAX] {
            let first = run(seed).expect("differential workload");
            equal(&first, &run(seed).expect("replay"), "reproducible-report").expect("determinism");
        }
    }

    #[test]
    fn explicit_batch_data_and_order_reach_the_oracles() {
        let key = B256::repeat_byte(0x10);
        let mut harness = TrieTestHarness::new(BTreeMap::from([(key, U256::ONE)]));
        let mut trie = ArenaParallelSparseTrie::default().with_parallelism_thresholds(
            ArenaParallelismThresholds {
                min_dirty_leaves: u64::MAX,
                min_revealed_nodes: usize::MAX,
                min_updates: usize::MAX,
                min_leaves_for_prune: u64::MAX,
            },
        );
        let mut evidence = Evidence::default();
        initialize(&mut trie, &harness, true, &mut evidence).unwrap();
        let change = |value: u64| Change { key: key.0, value: U256::from(value).to_be_bytes() };
        let actions = [
            Action::UpdateBatch { updates: vec![change(2), change(3)] },
            Action::Snapshot,
            Action::Clone,
            Action::TrackingReset,
            Action::Prune { cutoff: u64::MAX },
            Action::Reveal { key: key.0 },
            Action::UpdateBatch { updates: vec![] },
            Action::UpdateBatch { updates: vec![change(0)] },
        ];
        randomized_actions(&mut trie, &mut harness, &actions, 8, &mut evidence).unwrap();
        assert!(harness.storage().is_empty());
        assert_eq!(evidence.checkpoints[0].0, oracle(&BTreeMap::from([(key, U256::from(3))])));
        assert_eq!(evidence.checkpoints[1].0, evidence.checkpoints[0].0);
        assert_eq!(evidence.checkpoints[2].0, EMPTY_ROOT_HASH);
        // Deleting the last leaf does not prevent the next explicit insertion or proof.
        randomized_actions(
            &mut trie,
            &mut harness,
            &[Action::UpdateBatch { updates: vec![change(128)] }, Action::Reveal { key: key.0 }],
            9,
            &mut evidence,
        )
        .unwrap();
        assert_eq!(harness.storage(), &BTreeMap::from([(key, U256::from(128))]));
        assert_ne!(evidence.checkpoints[3].0, evidence.checkpoints[0].0);
    }
}
