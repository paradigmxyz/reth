//! Empirical validation of `memory_size()` against a counting allocator.
//!
//! Builds an `ArenaParallelSparseTrie`, populates it with leaves, hashes, prunes at
//! various retention ratios, and compares the reported `memory_size()` against
//! allocator-measured bytes.

use alloy_primitives::{keccak256, B256, U256};
use alloy_rlp::encode_fixed_size;
use reth_trie::test_utils::TrieTestHarness;
use reth_trie_common::{Nibbles, ProofV2Target};
use reth_trie_sparse::{ArenaParallelSparseTrie, LeafUpdate, SparseTrie};
use std::collections::BTreeMap;

use alloy_primitives::map::B256Map;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};

/// A simple counting allocator that wraps the system allocator.
struct CountingAlloc;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // old size subtracted, new size added
            if new_size > layout.size() {
                ALLOCATED.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            } else {
                ALLOCATED.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn allocated_bytes() -> usize {
    ALLOCATED.load(Ordering::Relaxed)
}

/// Helper: create a harness, init a trie, reveal all leaves.
fn build_trie(num_leaves: usize) -> (TrieTestHarness, ArenaParallelSparseTrie) {
    let storage: BTreeMap<B256, U256> = (0..num_leaves)
        .map(|i| {
            let key = keccak256(B256::from(U256::from(i)));
            let value = U256::from(i + 1);
            (key, value)
        })
        .collect();

    let harness = TrieTestHarness::new(storage.clone());

    let root_node = harness.root_node();
    let mut trie = ArenaParallelSparseTrie::default();
    trie.set_root(root_node.node, root_node.masks, false).expect("set_root");

    // Reveal all leaves.
    let keys: Vec<B256> = storage.keys().copied().collect();
    let mut targets: Vec<ProofV2Target> = keys.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes");

    // Insert leaf values via update_leaves.
    let mut leaf_updates: B256Map<LeafUpdate> = storage
        .iter()
        .map(|(&slot, &value)| (slot, LeafUpdate::Changed(encode_fixed_size(&value).to_vec())))
        .collect();

    loop {
        let mut proof_targets: Vec<ProofV2Target> = Vec::new();
        trie.update_leaves(&mut leaf_updates, |key, min_len| {
            proof_targets.push(ProofV2Target::new(key).with_min_len(min_len));
        })
        .expect("update_leaves");

        if proof_targets.is_empty() {
            break;
        }

        let (mut proof_nodes, _) = harness.proof_v2(&mut proof_targets);
        trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes");
    }

    (harness, trie)
}

#[test]
#[ignore = "expensive: requires ~50k leaves, run explicitly with --ignored"]
fn test_memory_size_vs_allocator() {
    // 50k leaves at 50% retention. At this scale, fixed per-trie overhead is
    // negligible and the ratio converges toward 1.0.
    let num_leaves = 50_000;
    let all_keys: Vec<B256> =
        (0..num_leaves).map(|i| keccak256(B256::from(U256::from(i)))).collect();

    let (_, mut trie) = build_trie(num_leaves);
    let _root = trie.root();

    let retain_count = num_leaves / 2;
    let retained: Vec<Nibbles> =
        all_keys[..retain_count].iter().map(|k| Nibbles::unpack(*k)).collect();

    trie.prune(&retained);

    let reported = trie.memory_size();
    let before_drop = allocated_bytes();
    drop(trie);
    let after_drop = allocated_bytes();
    let actual_freed = before_drop.saturating_sub(after_drop);

    let ratio = reported as f64 / actual_freed as f64;
    assert!(
        ratio > 0.90 && ratio < 1.10,
        "memory_size ratio {ratio:.2} outside [0.90, 1.10] \
         (memory_size={reported}, actual_freed={actual_freed})"
    );
}
