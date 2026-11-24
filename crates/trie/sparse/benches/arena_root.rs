//! Tempo-shaped sparse trie root benchmark.
//!
//! Builds `TOKENS` large, fully revealed storage tries (token-balance-like slots with random
//! hashed keys) plus an account trie, then simulates blocks: each block changes K storage
//! slots spread evenly across the tokens and K / 4 accounts, and computes every root.
//!
//! Phases are timed separately per block:
//! - upd: `update_leaves` on all storage tries and on the account trie,
//! - hash: `root()` on all storage tries (in parallel, as the engine does) plus the account trie.
//!
//! Configuration via environment variables:
//! - SLOTS (default 2,000,000): slots per token storage trie.
//! - ACCOUNTS (default 1,000,000): accounts in the account trie.
//! - TOKENS (default 4).
//! - KS (default 500,2000,5000,20000): dirty storage leaves per block.
//! - BLOCKS (default 20) measured blocks per K and mode, after `WARMUP` (default 3).
//! - MODES (default `depth_first,level_batched`): hashing strategies, interleaved per block.
//! - HOT (default 0): if set, each trie keeps only HOT hot keys revealed: after building, the hot
//!   keys are updated and every other node is pruned (blinded), as a long-running node's sparse
//!   tries look. Blocks then only touch hot keys. 0 keeps every node revealed.
//!
//! Run with:
//! `cargo bench -p reth-trie-sparse --bench arena_root --features alloy-primitives/asm-keccak`
#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{map::B256Map, B256, U256};
use alloy_trie::EMPTY_ROOT_HASH;
use rand::{rngs::StdRng, Rng, SeedableRng};
use rayon::prelude::*;
use reth_trie_common::{ProofTrieNode, TrieAccount};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, HashingStrategy, LeafUpdate, SparseTrie, TrieNodeEpoch,
};
use std::time::{Duration, Instant};

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.replace('_', "").parse().ok()).unwrap_or(default)
}

struct Trie {
    trie: ArenaParallelSparseTrie,
    keys: Vec<B256>,
}

fn new_trie() -> ArenaParallelSparseTrie {
    let mut trie = ArenaParallelSparseTrie::default();
    trie.set_root(&mut [ProofTrieNode::empty()], true).unwrap();
    trie
}

fn apply(trie: &mut ArenaParallelSparseTrie, updates: &mut B256Map<LeafUpdate>) {
    trie.update_leaves(updates, |key, _| panic!("trie is fully revealed, no proof for {key}"))
        .unwrap();
    assert!(updates.is_empty());
}

fn random_b256(rng: &mut StdRng) -> B256 {
    B256::from(rng.random::<[u8; 32]>())
}

fn storage_value(rng: &mut StdRng) -> Vec<u8> {
    alloy_rlp::encode(U256::from(rng.random::<u128>() | 1))
}

#[allow(clippy::needless_update)] // `TrieAccount` has more fields with some features.
fn account_value(rng: &mut StdRng, storage_root: B256) -> Vec<u8> {
    alloy_rlp::encode(TrieAccount {
        nonce: rng.random::<u32>() as u64,
        balance: U256::from(rng.random::<u128>()),
        storage_root,
        code_hash: random_b256(rng),
        ..Default::default()
    })
}

fn build(n: usize, hot: usize, seed: u64, value: impl Fn(&mut StdRng) -> Vec<u8>) -> Trie {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut trie = new_trie();
    let mut keys = Vec::with_capacity(n);
    let chunk = 250_000;
    let mut done = 0;
    while done < n {
        let m = chunk.min(n - done);
        let mut updates = B256Map::default();
        for _ in 0..m {
            let key = random_b256(&mut rng);
            keys.push(key);
            updates.insert(key, LeafUpdate::Changed(value(&mut rng)));
        }
        apply(&mut trie, &mut updates);
        done += m;
    }
    // Build with the depth-first path so that profiles of the level-batched path only see
    // per-block work.
    trie.set_hashing_strategy(HashingStrategy::DepthFirst);
    trie.root(TrieNodeEpoch::new(0));
    trie.take_updates();
    if hot > 0 {
        // Touch the hot keys at epoch 1 and blind everything older.
        keys.truncate(hot);
        let mut updates: B256Map<LeafUpdate> =
            keys.iter().map(|k| (*k, LeafUpdate::Changed(value(&mut rng)))).collect();
        apply(&mut trie, &mut updates);
        trie.root(TrieNodeEpoch::new(1));
        trie.take_updates();
        trie.prune(TrieNodeEpoch::new(1));
    }
    Trie { trie, keys }
}

#[derive(Default, Clone)]
struct Sample {
    upd: Duration,
    hash_storage: Duration,
    hash_account: Duration,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let slots = env_usize("SLOTS", 2_000_000);
    let accounts = env_usize("ACCOUNTS", 1_000_000);
    let tokens = env_usize("TOKENS", 4);
    let blocks = env_usize("BLOCKS", 20);
    let warmup = env_usize("WARMUP", 3);
    let hot = env_usize("HOT", 0);
    let ks: Vec<usize> = std::env::var("KS")
        .unwrap_or_else(|_| "500,2000,5000,20000".to_string())
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    let modes: Vec<HashingStrategy> = std::env::var("MODES")
        .unwrap_or_else(|_| "depth_first,level_batched".to_string())
        .split(',')
        .map(|s| match s.trim() {
            "depth_first" | "base" => HashingStrategy::DepthFirst,
            "level_batched" | "new" => HashingStrategy::LevelBatched,
            other => panic!("unknown mode {other}"),
        })
        .collect();

    let t0 = Instant::now();
    let mut storages: Vec<Trie> = (0..tokens)
        .into_par_iter()
        .map(|t| build(slots, hot, 1000 + t as u64, storage_value))
        .collect();
    let mut account = build(accounts, hot, 7, |rng| account_value(rng, EMPTY_ROOT_HASH));
    eprintln!(
        "built {tokens} x {slots} slots + {accounts} accounts (hot {hot}) in {:.1}s on {} threads",
        t0.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );

    let mut rng = StdRng::seed_from_u64(42);
    let mut epoch = 2u64;
    println!("K\tmode\tupd_ms\thash_storage_ms\thash_account_ms\thash_ms\ttotal_ms");
    for &k in &ks {
        let mut samples: Vec<Vec<Sample>> = vec![Vec::new(); modes.len()];
        for b in 0..(warmup + blocks) * modes.len() {
            let mi = b % modes.len();
            let mode = modes[mi];
            for s in &mut storages {
                s.trie.set_hashing_strategy(mode);
            }
            account.trie.set_hashing_strategy(mode);

            // Prepare updates outside the timed region.
            let per_token = k / tokens;
            let mut storage_updates: Vec<B256Map<LeafUpdate>> = storages
                .iter()
                .map(|s| {
                    let mut m = B256Map::default();
                    while m.len() < per_token {
                        let key = s.keys[rng.random_range(0..s.keys.len())];
                        m.insert(key, LeafUpdate::Changed(storage_value(&mut rng)));
                    }
                    m
                })
                .collect();
            let n_acc = k / 4;
            let mut account_keys = Vec::with_capacity(n_acc);
            while account_keys.len() < n_acc {
                account_keys.push(account.keys[rng.random_range(0..account.keys.len())]);
            }

            let start = Instant::now();
            storages.par_iter_mut().zip(storage_updates.par_iter_mut()).for_each(|(s, u)| {
                apply(&mut s.trie, u);
            });
            let upd_storage = start.elapsed();

            let new_epoch = TrieNodeEpoch::new(epoch);
            epoch += 1;
            let start = Instant::now();
            let roots: Vec<B256> =
                storages.par_iter_mut().map(|s| s.trie.root(new_epoch)).collect();
            let hash_storage = start.elapsed();

            let mut account_updates = B256Map::default();
            for (i, root) in roots.iter().enumerate() {
                account_updates
                    .insert(account.keys[i], LeafUpdate::Changed(account_value(&mut rng, *root)));
            }
            for key in &account_keys {
                account_updates
                    .insert(*key, LeafUpdate::Changed(account_value(&mut rng, EMPTY_ROOT_HASH)));
            }
            let start = Instant::now();
            apply(&mut account.trie, &mut account_updates);
            let upd = upd_storage + start.elapsed();

            let start = Instant::now();
            let state_root = account.trie.root(new_epoch);
            let hash_account = start.elapsed();
            std::hint::black_box(state_root);

            for s in &mut storages {
                std::hint::black_box(s.trie.take_updates());
            }
            std::hint::black_box(account.trie.take_updates());

            if b >= warmup * modes.len() {
                samples[mi].push(Sample { upd, hash_storage, hash_account });
            }
        }
        for (mi, mode) in modes.iter().enumerate() {
            let s = &samples[mi];
            let ms = |f: &dyn Fn(&Sample) -> Duration| {
                median(s.iter().map(|x| f(x).as_secs_f64() * 1e3).collect())
            };
            println!(
                "{k}\t{mode:?}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}",
                ms(&|x| x.upd),
                ms(&|x| x.hash_storage),
                ms(&|x| x.hash_account),
                ms(&|x| x.hash_storage + x.hash_account),
                ms(&|x| x.upd + x.hash_storage + x.hash_account),
            );
        }
    }
}
