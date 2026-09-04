//! Deterministic, bounded differential fuzzing of sparse trie updates and persistence.
//!
//! Run with `cargo run --release -p reth-trie --features test-utils
//! --example sparse_differential_fuzz -- --cases 10000 --seed 1 --mask-sweep`.
//! A failing sequence can be replayed using the printed `--case-seed`.

use alloy_primitives::{keccak256, map::B256Map, B256, U256};
use alloy_rlp::Decodable;
use alloy_trie::{nodes::TrieNode, proof::ProofRetainer, TrieMask};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_trie::{
    hashed_cursor::{
        mock::{MockHashedCursor, MockHashedCursorFactory},
        HashedCursorFactory,
    },
    proof_v2::StorageProofCalculator,
    test_utils::storage_root_prehashed,
    trie_cursor::{
        mock::{MockTrieCursor, MockTrieCursorFactory},
        TrieCursorFactory,
    },
};
use reth_trie_common::{
    triehash::KeccakHasher, BranchNodeCompact, HashBuilder, Nibbles, ProofV2Target,
};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, ArenaParallelismThresholds, LeafUpdate, SparseTrie, SparseTrieUpdates,
    TrieNodeEpoch,
};
use std::{collections::BTreeMap, time::Instant};

const VALUE_LENGTHS: [usize; 17] =
    [0, 1, 2, 26, 27, 28, 29, 30, 31, 32, 33, 54, 55, 56, 57, 127, 128];

#[derive(Default, Debug)]
struct Counts {
    sequences: u64,
    operations: u64,
    root_checks: u64,
    persistence_checks: u64,
    proof_rounds: u64,
    reopens: u64,
    prunes: u64,
    occupancy_masks: u64,
}

fn main() {
    let mut cases = 2000;
    let mut seed = 1_u64;
    let mut case_seed = None;
    let mut mask_sweep = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cases" => cases = args.next().expect("case count").parse().expect("integer"),
            "--seed" => seed = args.next().expect("seed").parse().expect("integer"),
            "--case-seed" => {
                case_seed = Some(args.next().expect("case seed").parse().expect("integer"));
            }
            "--mask-sweep" => mask_sweep = true,
            _ => panic!("unknown argument: {arg}"),
        }
    }
    let started = Instant::now();
    let mut counts = Counts::default();
    if mask_sweep {
        eprintln!("Checking all 65536 branch occupancy masks");
        check_occupancy_masks(&mut counts);
    }
    let mut seeds = StdRng::seed_from_u64(seed);
    for case in 0..if case_seed.is_some() { 1 } else { cases } {
        let current_seed = case_seed.unwrap_or_else(|| seeds.random());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_sequence(current_seed, &mut counts);
        }));
        if let Err(panic) = result {
            eprintln!("REPRODUCE: --case-seed {current_seed} (seed {seed}, case {case})");
            std::panic::resume_unwind(panic);
        }
        counts.sequences += 1;
        if counts.sequences.is_multiple_of(1000) {
            eprintln!("{} sequences in {:.2}s", counts.sequences, started.elapsed().as_secs_f64());
        }
    }
    println!("PASS seed={seed} elapsed={:.3}s {counts:?}", started.elapsed().as_secs_f64());
}

fn trie(parallel: bool) -> ArenaParallelSparseTrie {
    let threshold = if parallel { 1 } else { usize::MAX };
    let mut trie = ArenaParallelSparseTrie::default().with_parallelism_thresholds(
        ArenaParallelismThresholds {
            min_dirty_leaves: threshold as u64,
            min_revealed_nodes: threshold,
            min_updates: threshold,
            min_leaves_for_prune: threshold as u64,
        },
    );
    trie.set_updates(true);
    trie
}

const fn epoch(value: u64) -> TrieNodeEpoch {
    TrieNodeEpoch::new(value)
}

fn key_pool(rng: &mut StdRng) -> Vec<B256> {
    let prefix_len = rng.random_range(0..64);
    let common: [u8; 32] = rng.random();
    (0..64_u8)
        .map(|index| {
            let mut key = keccak256([index, common[31]]);
            for nibble in 0..prefix_len {
                let mask = if nibble % 2 == 0 { 0xf0 } else { 0x0f };
                key[nibble / 2] = (key[nibble / 2] & !mask) | (common[nibble / 2] & mask);
            }
            // Exercise every child position at the common prefix, including keys that differ
            // only in their final nibble (where leaves and branches can be inlined).
            let shift = if prefix_len % 2 == 0 { 4 } else { 0 };
            key[prefix_len / 2] = (key[prefix_len / 2] & !(15 << shift)) | ((index & 15) << shift);
            key
        })
        .collect()
}

fn apply_persisted(nodes: &mut BTreeMap<Nibbles, BranchNodeCompact>, updates: SparseTrieUpdates) {
    // Persistence gives replacements precedence when a path is removed and recreated in a batch.
    for path in updates.removed_nodes {
        nodes.remove(&path);
    }
    nodes.extend(updates.updated_nodes);
}

fn check_raw_root(
    trie: &mut ArenaParallelSparseTrie,
    state: &BTreeMap<B256, Vec<u8>>,
    persisted: &mut BTreeMap<Nibbles, BranchNodeCompact>,
    current_epoch: u64,
    counts: &mut Counts,
) {
    let expected = triehash::trie_root::<KeccakHasher, _, _, _>(state);
    assert_eq!(trie.root(epoch(current_epoch)), expected, "raw leaf root");
    let cached_epoch = trie.root_epoch();
    assert_eq!(trie.root(epoch(current_epoch + 1)), expected, "cached root");
    assert_eq!(trie.root_epoch(), cached_epoch, "reading a cached root must retain its epoch");
    counts.root_checks += 2;
    apply_persisted(persisted, trie.take_updates());

    let mut reference = HashBuilder::default()
        .with_proof_retainer(ProofRetainer::new(state.keys().map(Nibbles::unpack).collect()));
    for (key, value) in state {
        reference.add_leaf(Nibbles::unpack(key), value);
        assert_eq!(trie.get_leaf_value(&Nibbles::unpack(key)), Some(value));
    }
    assert_eq!(reference.root(), expected, "streaming reference root");
    let expected_nodes = compact_reference_nodes(&mut reference);
    assert_eq!(*persisted, expected_nodes, "retained compact branch nodes");
    counts.persistence_checks += 1;
}

/// Derives persistence metadata from independently encoded full proof nodes. `HashBuilder`'s
/// update collection assumes branch children are hashed, so it cannot serve as the oracle for
/// short keys and values whose child branches fit directly inside their parent's RLP.
fn compact_reference_nodes(reference: &mut HashBuilder) -> BTreeMap<Nibbles, BranchNodeCompact> {
    let mut metadata = BTreeMap::<Nibbles, (bool, bool)>::new();
    let mut compact = BTreeMap::new();
    for (path, encoded) in reference.take_proof_nodes().into_nodes_sorted().into_iter().rev() {
        let node = TrieNode::decode(&mut encoded.as_ref()).unwrap();
        let flags = match node {
            TrieNode::EmptyRoot | TrieNode::Leaf(_) => (false, false),
            TrieNode::Extension(extension) => {
                let child = metadata.get(&path.join(&extension.key)).expect("extension child");
                (false, child.1)
            }
            TrieNode::Branch(branch) => {
                let mut hash_mask = TrieMask::default();
                let mut tree_mask = TrieMask::default();
                let mut hashes = Vec::new();
                for (nibble, child) in branch.state_mask.iter().zip(&branch.stack) {
                    let mut child_path = path;
                    child_path.push(nibble);
                    let (is_branch, has_descendants) = metadata[&child_path];
                    if is_branch && child.is_hash() {
                        hash_mask.set_bit(nibble);
                        hashes.push(child.as_hash().unwrap());
                    }
                    if has_descendants {
                        tree_mask.set_bit(nibble);
                    }
                }
                let retained = !hash_mask.is_empty() || !tree_mask.is_empty();
                if retained && !path.is_empty() {
                    compact.insert(
                        path,
                        BranchNodeCompact::new(
                            branch.state_mask,
                            tree_mask,
                            hash_mask,
                            hashes,
                            None,
                        ),
                    );
                }
                (true, retained)
            }
        };
        metadata.insert(path, flags);
    }
    compact
}

fn check_sequence(seed: u64, counts: &mut Counts) {
    let mut rng = StdRng::seed_from_u64(seed);
    let keys = key_pool(&mut rng);
    let parallel = rng.random();
    let mut raw = trie(parallel);
    let mut raw_state = BTreeMap::<B256, Vec<u8>>::new();
    let mut raw_nodes = BTreeMap::new();
    for batch in 0..8 {
        let mut updates = B256Map::default();
        for _ in 0..rng.random_range(1..=32) {
            let key = keys[rng.random_range(0..keys.len())];
            let value = match rng.random_range(0..8) {
                0 | 1 => Vec::new(),
                2 => raw_state.get(&key).cloned().unwrap_or_default(),
                _ => {
                    let len = VALUE_LENGTHS[rng.random_range(0..VALUE_LENGTHS.len())];
                    let payload: Vec<u8> = (0..len).map(|_| rng.random()).collect();
                    alloy_rlp::encode(payload.as_slice())
                }
            };
            if value.is_empty() {
                raw_state.remove(&key);
            } else {
                raw_state.insert(key, value.clone());
            }
            updates.insert(key, LeafUpdate::Changed(value));
            counts.operations += 1;
        }
        updates.entry(keys[rng.random_range(0..keys.len())]).or_insert(LeafUpdate::Touched);
        counts.operations += 1;
        raw.update_leaves(&mut updates, |_, _| panic!("fully revealed trie requested a proof"))
            .unwrap();
        assert!(updates.is_empty());
        check_raw_root(&mut raw, &raw_state, &mut raw_nodes, batch * 2 + 1, counts);
    }

    // Clear and reuse allocated arenas after arbitrary mutations, then rebuild in reverse order.
    raw.clear();
    raw.set_updates(true);
    raw_nodes.clear();
    for (key, value) in raw_state.iter().rev() {
        let mut updates = std::iter::once((*key, LeafUpdate::Changed(value.clone()))).collect();
        raw.update_leaves(&mut updates, |_, _| panic!("cleared trie requested a proof")).unwrap();
        counts.operations += 1;
    }
    check_raw_root(&mut raw, &raw_state, &mut raw_nodes, 20, counts);
    let mut deletions =
        raw_state.keys().map(|key| (*key, LeafUpdate::Changed(Vec::new()))).collect();
    counts.operations += raw_state.len() as u64;
    raw.update_leaves(&mut deletions, |_, _| panic!("rebuilt trie requested a proof")).unwrap();
    raw_state.clear();
    check_raw_root(&mut raw, &raw_state, &mut raw_nodes, 22, counts);

    check_storage_sequence(&mut rng, &keys, parallel, counts);
}

fn check_storage_sequence(rng: &mut StdRng, keys: &[B256], parallel: bool, counts: &mut Counts) {
    let mut storage = BTreeMap::new();
    let mut persisted = BTreeMap::new();
    let mut sparse = trie(parallel);

    for batch in 0..9 {
        let mut changes = BTreeMap::new();
        for _ in 0..if batch == 8 { 0 } else { rng.random_range(1..=32) } {
            let key = keys[rng.random_range(0..keys.len())];
            let value = match rng.random_range(0..8) {
                0 | 1 => U256::ZERO,
                2 => *storage.get(&key).unwrap_or(&U256::ZERO),
                3 => U256::from(rng.random::<u8>()),
                _ => U256::from_be_bytes(rng.random::<[u8; 32]>()),
            };
            changes.insert(key, value);
            counts.operations += 1;
        }
        if batch == 8 {
            changes.extend(storage.keys().map(|key| (*key, U256::ZERO)));
            counts.operations += storage.len() as u64;
        }
        let mut updates: B256Map<_> = changes
            .iter()
            .map(|(key, value)| {
                let encoded = if value.is_zero() { Vec::new() } else { alloy_rlp::encode(value) };
                (*key, LeafUpdate::Changed(encoded))
            })
            .collect();
        updates.entry(keys[rng.random_range(0..keys.len())]).or_insert(LeafUpdate::Touched);
        counts.operations += 1;

        let mut converged = false;
        for _ in 0..=keys.len() * 2 {
            let mut targets = Vec::new();
            sparse
                .update_leaves(&mut updates, |key, parent| {
                    targets.push(ProofV2Target::new(key).with_parent(parent));
                })
                .unwrap();
            if targets.is_empty() {
                assert!(updates.is_empty());
                converged = true;
                break;
            }
            let mut nodes = storage_proof_calculator(&storage, &persisted)
                .storage_proof(B256::ZERO, &mut targets)
                .unwrap();
            sparse.reveal_nodes(&mut nodes).unwrap();
            counts.proof_rounds += 1;
        }
        assert!(converged, "proof/update loop failed to make progress");

        for (key, value) in &changes {
            if value.is_zero() {
                storage.remove(key);
            } else {
                storage.insert(*key, *value);
            }
        }
        let expected_root = storage_root_prehashed(storage.iter().map(|(k, v)| (*k, *v)));
        let current_epoch = batch * 3 + 1;
        assert_eq!(sparse.root(epoch(current_epoch)), expected_root, "storage root");
        let cached_epoch = sparse.root_epoch();
        assert_eq!(sparse.root(epoch(current_epoch + 1)), expected_root);
        assert_eq!(sparse.root_epoch(), cached_epoch);
        counts.root_checks += 2;
        apply_persisted(&mut persisted, sparse.take_updates());

        let mut reference = HashBuilder::default()
            .with_proof_retainer(ProofRetainer::new(storage.keys().map(Nibbles::unpack).collect()));
        for (key, value) in &storage {
            reference.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode(value));
        }
        assert_eq!(reference.root(), expected_root);
        let expected_nodes = compact_reference_nodes(&mut reference);
        assert_eq!(persisted, expected_nodes, "persisted storage compact nodes");
        counts.persistence_checks += 1;

        // Reopen through proof cursors backed by the accumulated sparse updates, so subsequent
        // proof generation consumes the persisted representation being verified.
        let root =
            storage_proof_calculator(&storage, &persisted).storage_root_node(B256::ZERO).unwrap();
        let mut reopened = trie(parallel);
        reopened.set_root(root.node, root.masks, true).unwrap();
        assert_eq!(reopened.root(epoch(current_epoch + 1)), expected_root, "reopened storage root");
        counts.root_checks += 1;
        counts.reopens += 1;
        if batch % 2 == 0 {
            sparse = reopened;
        } else {
            sparse.prune(epoch(current_epoch + 1));
            assert_eq!(sparse.root(epoch(current_epoch + 2)), expected_root, "pruned storage root");
            counts.root_checks += 1;
            counts.prunes += 1;
        }
    }
}

fn storage_proof_calculator(
    storage: &BTreeMap<B256, U256>,
    persisted: &BTreeMap<Nibbles, BranchNodeCompact>,
) -> StorageProofCalculator<MockTrieCursor, MockHashedCursor<U256>> {
    let trie = MockTrieCursorFactory::new(
        BTreeMap::new(),
        std::iter::once((B256::ZERO, persisted.clone())).collect(),
    );
    let hashed = MockHashedCursorFactory::new(
        BTreeMap::new(),
        std::iter::once((B256::ZERO, storage.clone())).collect(),
    );
    StorageProofCalculator::new_storage(
        trie.storage_trie_cursor(B256::ZERO).unwrap(),
        hashed.hashed_storage_cursor(B256::ZERO).unwrap(),
    )
}

fn check_occupancy_masks(counts: &mut Counts) {
    for mask in 0..=u16::MAX {
        let mut sparse = trie(false);
        let mut state = BTreeMap::new();
        let prefix = usize::from(mask) % 64;
        for nibble in 0..16_u8 {
            if mask & (1 << nibble) == 0 {
                continue;
            }
            let mut key = B256::ZERO;
            key[prefix / 2] = if prefix % 2 == 0 { nibble << 4 } else { nibble };
            let len = VALUE_LENGTHS[usize::from(mask) % VALUE_LENGTHS.len()];
            state.insert(key, alloy_rlp::encode(vec![nibble; len].as_slice()));
        }
        let mut updates =
            state.iter().map(|(key, value)| (*key, LeafUpdate::Changed(value.clone()))).collect();
        sparse
            .update_leaves(&mut updates, |_, _| panic!("occupancy sweep requested proof"))
            .unwrap();
        counts.operations += state.len() as u64;
        check_raw_root(&mut sparse, &state, &mut BTreeMap::new(), 1, counts);
        counts.occupancy_masks += 1;
    }
}
