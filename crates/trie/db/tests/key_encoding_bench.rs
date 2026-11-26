//! Compares the legacy, packed and memcmp trie key encodings on a synthetic state.
//!
//! Ignored by default. Run with:
//!
//! ```sh
//! cargo test --release -p reth-trie-db --test key_encoding_bench -- --ignored --nocapture
//! ```
//!
//! `BENCH_ACCOUNTS` (default 2,000,000), `BENCH_SLOT_SCALE` (default 1, multiplies every
//! storage tier) and `BENCH_REPS` (default 5) size the run.

#![allow(missing_docs, clippy::field_reassign_with_default)]

use alloy_primitives::{keccak256, B256, U256};
use reth_chainspec::MAINNET;
use reth_db::{mdbx::DatabaseArguments, tables};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRW},
    database::Database,
    table::{Compress, Decode, Decompress, Encode, Table},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::{
    test_utils::{create_test_provider_factory_with_chain_spec_and_db_args, MockNodeTypesWithDB},
    ProviderFactory, StateWriter,
};
use reth_trie::{
    prefix_set::TriePrefixSets,
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::TrieUpdatesSorted,
    BranchNodeCompact, HashedPostState, Nibbles, StateRoot,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseStorageTrieCursor,
    DatabaseTrieCursorFactory, LegacyKeyAdapter, MemcmpKeyAdapter, PackedKeyAdapter,
    StorageTrieEntryLike, TrieTableAdapter,
};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

type DbStateRoot<'a, TX, A> =
    StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
type Factory = ProviderFactory<MockNodeTypesWithDB>;

const SAMPLE: usize = 200_000;

fn env_or(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Deterministic xorshift64 so every adapter sees the same inputs.
struct Rng(u64);

impl Rng {
    const fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    const fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

fn hashed(tag: u8, i: u64) -> B256 {
    let mut buf = [0u8; 9];
    buf[0] = tag;
    buf[1..].copy_from_slice(&i.to_be_bytes());
    keccak256(buf)
}

fn account(i: u64, contract: bool) -> Account {
    let mut account = Account::default();
    account.nonce = i;
    account.balance = U256::from(i + 1);
    account.bytecode_hash = contract.then(|| hashed(7, i));
    account
}

/// Synthetic state: `accounts` hashed accounts, the first of which hold storage in tiers that
/// roughly follow mainnet's skew (a few huge contracts, many tiny ones).
struct State {
    accounts: Vec<(B256, Account)>,
    /// `(hashed address, sorted slots)`, sorted by address.
    storages: Vec<(B256, Vec<B256>)>,
}

impl State {
    fn new(accounts: u64, slot_scale: u64) -> Self {
        let tiers = [(1, 1_000_000), (10, 100_000), (100, 5_000), (20_000, 20)];
        let mut contract = 0u64;
        let mut storages = Vec::new();
        for (count, slots) in tiers {
            for _ in 0..count {
                let mut keys: Vec<_> =
                    (0..slots * slot_scale).map(|s| hashed(2, (contract << 32) | s)).collect();
                keys.sort_unstable();
                storages.push((hashed(0, contract), keys));
                contract += 1;
            }
        }
        storages.sort_unstable_by_key(|(address, _)| *address);

        let mut accounts: Vec<_> =
            (0..accounts.max(contract)).map(|i| (hashed(0, i), account(i, i < contract))).collect();
        accounts.sort_unstable_by_key(|(address, _)| *address);
        Self { accounts, storages }
    }

    fn slots(&self) -> usize {
        self.storages.iter().map(|(_, slots)| slots.len()).sum()
    }

    /// A block-sized change set: account updates and creations, slot updates, creations and
    /// deletions spread over every storage tier.
    fn change_set(&self, rng: &mut Rng) -> HashedPostState {
        let mut post_state = HashedPostState::default();
        for _ in 0..20_000 {
            let (address, account) = &self.accounts[rng.below(self.accounts.len())];
            let mut account = account.clone();
            account.balance += U256::from(1);
            post_state.accounts.insert(*address, Some(account));
        }
        for i in 0..2_000 {
            post_state.accounts.insert(hashed(9, i), Some(account(i, false)));
        }
        for (address, slots) in &self.storages {
            // ~0.5% of each storage trie, at least one slot.
            let changes = (slots.len() / 200).max(1);
            let storage = post_state.storages.entry(*address).or_default();
            for j in 0..changes {
                let value = match j % 10 {
                    0 => U256::ZERO,
                    _ => U256::from(rng.next() | 1),
                };
                storage.storage.insert(slots[rng.below(slots.len())], value);
            }
            storage.storage.insert(hashed(3, rng.next()), U256::from(1));
            let account = self.accounts.binary_search_by_key(address, |(a, _)| *a).unwrap();
            post_state.accounts.insert(*address, Some(self.accounts[account].1.clone()));
        }
        post_state
    }
}

fn new_factory() -> Factory {
    let args = DatabaseArguments::test().with_geometry_max_size(Some(16 << 30));
    create_test_provider_factory_with_chain_spec_and_db_args(MAINNET.clone(), args)
}

/// Writes the hashed state, computes the full state root through adapter `A` and stores the
/// trie. Returns the root and how long the root computation took.
fn build<A: TrieTableAdapter>(factory: &Factory, state: &State) -> (B256, Duration) {
    let provider = factory.provider_rw().unwrap();
    let tx = provider.tx_ref();
    {
        let mut cursor = tx.cursor_write::<tables::HashedAccounts>().unwrap();
        for (address, account) in &state.accounts {
            cursor.append(*address, account).unwrap();
        }
        let mut cursor = tx.cursor_dup_write::<tables::HashedStorages>().unwrap();
        for (address, slots) in &state.storages {
            for (i, key) in slots.iter().enumerate() {
                let entry = StorageEntry { key: *key, value: U256::from(i + 1) };
                cursor.append_dup(*address, entry).unwrap();
            }
        }
    }

    let start = Instant::now();
    let (root, updates) = DbStateRoot::<_, A>::from_tx(tx).root_with_updates().unwrap();
    let elapsed = start.elapsed();
    write_trie_updates::<A>(tx, &updates.into_sorted());
    provider.commit().unwrap();
    (root, elapsed)
}

/// Writes `updates` through adapter `A`, like `TrieWriter::write_trie_updates`.
fn write_trie_updates<A: TrieTableAdapter>(
    tx: &(impl DbTx + DbTxMut),
    updates: &TrieUpdatesSorted,
) {
    let mut cursor = tx.cursor_write::<A::AccountTrieTable>().unwrap();
    for (path, node) in updates.account_nodes_ref() {
        if let Some(node) = node.as_ref().filter(|_| !path.is_empty()) {
            cursor.upsert((*path).into(), node).unwrap();
        }
    }
    for (address, storage_updates) in updates.storage_tries_ref() {
        let cursor = tx.cursor_dup_write::<A::StorageTrieTable>().unwrap();
        DatabaseStorageTrieCursor::<_, A>::new(cursor, *address)
            .write_storage_trie_updates_sorted(storage_updates)
            .unwrap();
    }
}

struct TableStats {
    entries: usize,
    depth: u32,
    pages: usize,
    bytes: usize,
}

fn table_stats<T: Table>(factory: &Factory) -> TableStats {
    let tx = factory.db_ref().tx().unwrap();
    let db = tx.inner().open_db(Some(T::NAME)).unwrap();
    let stat = tx.inner().db_stat(db.dbi()).unwrap();
    let pages = stat.leaf_pages() + stat.branch_pages() + stat.overflow_pages();
    TableStats {
        entries: stat.entries(),
        depth: stat.depth(),
        pages,
        bytes: pages * stat.page_size() as usize,
    }
}

/// Stored trie paths, read through the legacy adapter.
struct Paths {
    account: Vec<Nibbles>,
    storage: Vec<(B256, Nibbles)>,
}

fn stored_paths(factory: &Factory) -> Paths {
    let tx = factory.db_ref().tx().unwrap();
    let cursors = DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx);
    let mut account = Vec::new();
    let mut cursor = cursors.account_trie_cursor().unwrap();
    let mut entry = cursor.seek(Nibbles::default()).unwrap();
    while let Some((path, _)) = entry {
        account.push(path);
        entry = cursor.next().unwrap();
    }
    let mut storage = Vec::new();
    let mut cursor = tx.cursor_dup_read::<tables::StoragesTrie>().unwrap();
    let mut walker = cursor.walk(None).unwrap();
    while let Some(Ok((address, entry))) = walker.next() {
        storage.push((address, entry.nibbles.0));
    }
    Paths { account, storage }
}

/// Runs `f` `reps` times and returns the median duration divided by `ops`, in nanoseconds.
fn median_ns(reps: u64, ops: usize, mut f: impl FnMut()) -> f64 {
    let mut runs: Vec<_> = (0..reps)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    runs.sort_unstable();
    runs[runs.len() / 2].as_nanos() as f64 / ops.max(1) as f64
}

/// Step 4: encode and decode cost on the real path distribution, without MDBX.
fn codec_ns<A: TrieTableAdapter>(paths: &Paths, reps: u64) -> [f64; 4] {
    let account_keys: Vec<Vec<u8>> =
        paths.account.iter().map(|p| A::AccountKey::from(*p).encode().as_ref().to_vec()).collect();
    // Node bytes don't depend on the key encoding, so an empty node isolates the key cost.
    let node = BranchNodeCompact::new(0b11, 0, 0, vec![], None);
    let values: Vec<Vec<u8>> = paths
        .storage
        .iter()
        .map(|(_, p)| {
            A::StorageValue::new(A::StorageSubKey::from(*p), node.clone()).compress().into()
        })
        .collect();

    let key_encode = median_ns(reps, paths.account.len(), || {
        for path in &paths.account {
            black_box(A::AccountKey::from(black_box(*path)).encode());
        }
    });
    let key_decode = median_ns(reps, account_keys.len(), || {
        for key in &account_keys {
            black_box(A::account_key_to_nibbles(&A::AccountKey::decode(black_box(key)).unwrap()));
        }
    });
    let mut buf = Vec::with_capacity(128);
    let value_encode = median_ns(reps, paths.storage.len(), || {
        for (_, path) in &paths.storage {
            buf.clear();
            A::StorageValue::new(A::StorageSubKey::from(black_box(*path)), node.clone())
                .compress_to_buf(&mut buf);
            black_box(&buf);
        }
    });
    let value_decode = median_ns(reps, values.len(), || {
        for value in &values {
            let entry = A::StorageValue::decompress(black_box(value)).unwrap();
            black_box(A::subkey_to_nibbles(entry.nibbles()));
        }
    });
    [key_encode, key_decode, value_encode, value_decode]
}

fn avg_key_bytes<A: TrieTableAdapter>(paths: &Paths) -> (f64, f64) {
    let account: usize =
        paths.account.iter().map(|p| A::AccountKey::from(*p).encode().as_ref().len()).sum();
    let storage: usize =
        paths.storage.iter().map(|(_, p)| A::StorageSubKey::from(*p).encode().as_ref().len()).sum();
    (account as f64 / paths.account.len() as f64, storage as f64 / paths.storage.len() as f64)
}

struct Targets {
    account_exact: Vec<Nibbles>,
    account_random: Vec<Nibbles>,
    storage_exact: Vec<(B256, Nibbles)>,
    storage_addresses: Vec<B256>,
}

impl Targets {
    fn new(paths: &Paths, state: &State, rng: &mut Rng) -> Self {
        let mut account_exact = paths.account.clone();
        rng.shuffle(&mut account_exact);
        account_exact.truncate(SAMPLE);
        let account_random = (0..SAMPLE)
            .map(|_| {
                let len = 1 + rng.below(8);
                let nibbles: Vec<u8> = (0..len).map(|_| (rng.next() & 0xf) as u8).collect();
                Nibbles::from_nibbles_unchecked(nibbles)
            })
            .collect();
        let mut storage_exact = paths.storage.clone();
        rng.shuffle(&mut storage_exact);
        storage_exact.truncate(SAMPLE);
        let storage_addresses = state.storages.iter().map(|(address, _)| *address).collect();
        Self { account_exact, account_random, storage_exact, storage_addresses }
    }
}

/// Step 5: cursor costs through adapter `A` on a warm page cache, in ns per operation.
fn cursor_ns<A: TrieTableAdapter>(factory: &Factory, targets: &Targets, reps: u64) -> [f64; 5] {
    let tx = factory.db_ref().tx().unwrap();
    let cursors = DatabaseTrieCursorFactory::<_, A>::new(&tx);

    let mut walked = 0;
    let account_walk = {
        let mut cursor = cursors.account_trie_cursor().unwrap();
        let mut count = 0;
        let ns = median_ns(reps, 1, || {
            count = 0;
            let mut entry = cursor.seek(Nibbles::default()).unwrap();
            while let Some(e) = entry {
                black_box(e);
                count += 1;
                entry = cursor.next().unwrap();
            }
        });
        walked += count;
        ns / count as f64
    };
    let account_exact = {
        let mut cursor = cursors.account_trie_cursor().unwrap();
        median_ns(reps, targets.account_exact.len(), || {
            for path in &targets.account_exact {
                black_box(cursor.seek_exact(*path).unwrap()).unwrap();
            }
        })
    };
    let account_random = {
        let mut cursor = cursors.account_trie_cursor().unwrap();
        median_ns(reps, targets.account_random.len(), || {
            for path in &targets.account_random {
                black_box(cursor.seek(*path).unwrap());
            }
        })
    };
    let storage_walk = {
        let mut cursor = cursors.storage_trie_cursor(B256::ZERO).unwrap();
        let mut count = 0;
        let ns = median_ns(reps, 1, || {
            count = 0;
            for address in &targets.storage_addresses {
                cursor.set_hashed_address(*address);
                let mut entry = cursor.seek(Nibbles::default()).unwrap();
                while let Some(e) = entry {
                    black_box(e);
                    count += 1;
                    entry = cursor.next().unwrap();
                }
            }
        });
        walked += count;
        ns / count as f64
    };
    let storage_exact = {
        let mut cursor = cursors.storage_trie_cursor(B256::ZERO).unwrap();
        median_ns(reps, targets.storage_exact.len(), || {
            for (address, path) in &targets.storage_exact {
                cursor.set_hashed_address(*address);
                black_box(cursor.seek_exact(*path).unwrap()).unwrap();
            }
        })
    };
    assert!(walked > 0);
    [account_walk, account_exact, account_random, storage_walk, storage_exact]
}

/// Step 5: incremental state root after `post_state`, through adapter `A`. The change set is
/// written in a transaction that is never committed, so every repetition sees the same state.
fn incremental_root<A: TrieTableAdapter>(
    factory: &Factory,
    post_state: &HashedPostState,
    prefix_sets: &TriePrefixSets,
    reps: u64,
) -> (B256, f64) {
    let provider = factory.provider_rw().unwrap();
    provider.write_hashed_state(&post_state.clone_into_sorted()).unwrap();
    let tx = provider.tx_ref();
    let mut root = B256::ZERO;
    let ns = median_ns(reps, 1, || {
        root =
            DbStateRoot::<_, A>::from_tx(tx).with_prefix_sets(prefix_sets.clone()).root().unwrap();
    });
    (root, ns)
}

macro_rules! per_adapter {
    ($f:ident($($arg:expr),*)) => {
        [
            $f::<LegacyKeyAdapter>($($arg),*),
            $f::<PackedKeyAdapter>($($arg),*),
            $f::<MemcmpKeyAdapter>($($arg),*),
        ]
    };
}

fn print_row(name: &str, values: [f64; 3], unit: &str) {
    let [legacy, packed, memcmp] = values;
    println!(
        "| {name} | {legacy:.1} {unit} | {packed:.1} {unit} | {memcmp:.1} {unit} | {:+.1}% |",
        (memcmp / packed - 1.0) * 100.0
    );
}

#[test]
#[ignore = "benchmark: run in release with --ignored --nocapture"]
fn key_encoding_bench() {
    let accounts = env_or("BENCH_ACCOUNTS", 2_000_000);
    let slot_scale = env_or("BENCH_SLOT_SCALE", 1);
    let reps = env_or("BENCH_REPS", 5);

    let state = State::new(accounts, slot_scale);
    println!(
        "state: {} accounts, {} storage tries, {} slots",
        state.accounts.len(),
        state.storages.len(),
        state.slots()
    );

    let factories = [new_factory(), new_factory(), new_factory()];
    let builds = [
        build::<LegacyKeyAdapter>(&factories[0], &state),
        build::<PackedKeyAdapter>(&factories[1], &state),
        build::<MemcmpKeyAdapter>(&factories[2], &state),
    ];
    assert!(builds.iter().all(|(root, _)| *root == builds[0].0), "full roots differ");
    println!("full root {} (legacy/packed/memcmp: {:?})", builds[0].0, builds.map(|b| b.1));

    let paths = stored_paths(&factories[0]);
    println!(
        "stored trie nodes: {} account, {} storage\n",
        paths.account.len(),
        paths.storage.len()
    );

    println!("| metric | legacy | packed (v2) | memcmp | memcmp vs packed |");
    println!("|---|---|---|---|---|");
    let key_bytes = per_adapter!(avg_key_bytes(&paths));
    print_row("avg account key", key_bytes.map(|k| k.0), "B");
    print_row("avg storage subkey", key_bytes.map(|k| k.1), "B");
    for (i, name) in ["AccountsTrie", "StoragesTrie"].into_iter().enumerate() {
        let stats = factories.each_ref().map(|f| {
            if i == 0 {
                table_stats::<tables::AccountsTrie>(f)
            } else {
                table_stats::<tables::StoragesTrie>(f)
            }
        });
        assert!(stats.iter().all(|s| s.entries == stats[0].entries));
        print_row(&format!("{name} size"), stats.each_ref().map(|s| s.bytes as f64 / 1e6), "MB");
        print_row(&format!("{name} pages"), stats.each_ref().map(|s| s.pages as f64), "");
        print_row(&format!("{name} depth"), stats.each_ref().map(|s| s.depth as f64), "");
    }

    let codec = per_adapter!(codec_ns(&paths, reps));
    for (i, name) in
        ["account key encode", "account key decode", "storage entry encode", "storage entry decode"]
            .into_iter()
            .enumerate()
    {
        print_row(name, codec.map(|c| c[i]), "ns");
    }

    let mut rng = Rng(0x5eed);
    let targets = Targets::new(&paths, &state, &mut rng);
    let cursor = [
        cursor_ns::<LegacyKeyAdapter>(&factories[0], &targets, reps),
        cursor_ns::<PackedKeyAdapter>(&factories[1], &targets, reps),
        cursor_ns::<MemcmpKeyAdapter>(&factories[2], &targets, reps),
    ];
    for (i, name) in [
        "account walk (per node)",
        "account seek_exact",
        "account seek (random)",
        "storage walk (per node)",
        "storage seek_exact",
    ]
    .into_iter()
    .enumerate()
    {
        print_row(name, cursor.map(|c| c[i]), "ns");
    }

    let post_state = state.change_set(&mut rng);
    let prefix_sets = post_state.construct_prefix_sets().freeze();
    let roots = [
        incremental_root::<LegacyKeyAdapter>(&factories[0], &post_state, &prefix_sets, reps),
        incremental_root::<PackedKeyAdapter>(&factories[1], &post_state, &prefix_sets, reps),
        incremental_root::<MemcmpKeyAdapter>(&factories[2], &post_state, &prefix_sets, reps),
    ];
    assert!(roots.iter().all(|(root, _)| *root == roots[0].0), "incremental roots differ");
    print_row("incremental state root", roots.map(|r| r.1 / 1e6), "ms");
    println!("\nmedian of {reps} runs, warm page cache");
}
