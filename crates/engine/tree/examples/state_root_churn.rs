//! Mainnet state-root capacity experiment. See state_root_churn.md before running.
//!
//! Compile the production task directly, without widening its public API or maintaining a copy.

use alloy_consensus::{Block, BlockHeader, Header};
use alloy_primitives::{map::B256Set, B256, U256};
use eyre::{ensure, Result};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use rand::{rngs::StdRng, seq::index::sample, Rng, SeedableRng};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::MAINNET;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    mdbx::{DatabaseArguments, MaxReadTransactionDuration},
    tables,
    transaction::DbTx,
    Database,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{Account, RecoveredBlock};
use reth_provider::{
    providers::{RocksDBProvider, StaticFileProvider},
    test_utils::MockNodeTypesWithDB,
    HeaderProvider, ProviderFactory, SaveBlocksInput,
};
use reth_prune_types::{PruneMode, PruneModes};
use reth_tasks::{RayonConfig, Runtime, RuntimeBuilder, RuntimeConfig, TokioConfig};
use reth_trie::{
    proof_v3::ProofCalculator,
    state_trie_cursor::{StateTrieCursor, StateTrieCursorFactory},
    HashedPostState, HashedStorage, Nibbles, StateTrieNode,
};
use reth_trie_parallel::{
    proof_task::{ProofTaskCtx, ProofWorkerHandle},
    state_root_task::{evm_state_to_hashed_post_state, StateRootComputeOutcome, StateRootMessage},
};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, RevealableSparseTrie, SparseStateTrie, TrieNodeEpoch,
};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

#[path = "../src/tree/state_root_strategy/sparse_trie.rs"]
mod task;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

const HOT: usize = 200_000;
const UPDATES: usize = 100_000;
type Trie = SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie>;
type Factory = ProviderFactory<MockNodeTypesWithDB<reth_db::DatabaseEnv>>;

#[derive(Clone)]
struct Entry {
    address: B256,
    account: Account,
    slots: [B256; 3],
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() >= 4, "sample DATADIR CORPUS COUNT | run DATADIR CORPUS CHURN BLOCKS WARMUP MASK OUTPUT [PERIOD_MS [PERSISTENCE_THRESHOLD [CACHE_GIB]]]");
    if args[0] == "sample" {
        return sample_mainnet(&args[1], &args[2], args[3].parse()?);
    }
    ensure!(args[0] == "run" && args.len() >= 8, "invalid run arguments");
    let churn: usize = args[3].parse()?;
    let blocks: usize = args[4].parse()?;
    let warmup: usize = args[5].parse()?;
    let mask: u64 = args[6].parse()?;
    let period = Duration::from_millis(args.get(8).map_or(Ok(1000), |s| s.parse())?);
    let persistence_threshold = args.get(9).map_or(Ok(mask + 10), |s| s.parse())?;
    let cache_gib: usize = args.get(10).map_or(Ok(12), |s| s.parse())?;
    ensure!(
        churn <= 50 && mask > 0 && blocks > 0 && persistence_threshold >= 5 && cache_gib > 0,
        "invalid experiment parameters"
    );
    std::fs::create_dir_all(&args[7])?;
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install()?;
    let runtime = RuntimeBuilder::new(RuntimeConfig {
        tokio: TokioConfig::with_worker_threads(4),
        rayon: RayonConfig {
            cpu_threads: Some(16),
            proof_account_worker_threads: Some(16),
            proof_storage_worker_threads: Some(64),
            rpc_threads: Some(2),
            storage_threads: Some(4),
            prewarming_threads: Some(2),
            bal_streaming_threads: Some(2),
            ..Default::default()
        },
    })
    .build()?;
    let result = run(
        &runtime,
        &snapshotter,
        &args[1],
        &args[2],
        churn,
        blocks,
        warmup,
        mask,
        persistence_threshold,
        cache_gib,
        period,
        Path::new(&args[7]),
    );
    runtime.shutdown_timeout(Duration::from_secs(30));
    result
}

fn sample_mainnet(datadir: &str, output: &str, count: usize) -> Result<()> {
    let db = reth_db::open_db_read_only(
        Path::new(datadir).join("db"),
        DatabaseArguments::default()
            .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded)),
    )?;
    let tx = db.tx()?;
    let finish = tx.get::<tables::StageCheckpoints>("Finish".into())?.unwrap();
    ensure!(
        finish
            .finish_stage_checkpoint()
            .and_then(|c| c.partial_state_trie())
            .unwrap_or(finish.block_number) ==
            finish.block_number,
        "snapshot must be fully persisted"
    );
    drop(tx);
    let mut seen = B256Set::default();
    let mut writer = BufWriter::new(File::create(output)?);
    writer.write_all(&(count as u64).to_le_bytes())?;
    let start = Instant::now();
    let mut batch = 0;
    while seen.len() < count {
        // Independent seeded chunks provide random I/O concurrency while deterministic joining
        // preserves corpus order regardless of worker scheduling.
        let chunks = thread::scope(|scope| -> Result<Vec<Vec<Entry>>> {
            let handles: Vec<_> = (0..16)
                .map(|worker| {
                    let db = &db;
                    scope.spawn(move || -> Result<Vec<Entry>> {
                        let tx = db.tx()?;
                        let mut accounts = tx.cursor_read::<tables::HashedAccounts>()?;
                        let mut storage = tx.cursor_dup_read::<tables::HashedStorages>()?;
                        let mut rng = StdRng::seed_from_u64(20261005 + batch * 16 + worker);
                        let mut entries = Vec::with_capacity(8192);
                        while entries.len() < 8192 {
                            let Some((address, first)) =
                                storage.seek(B256::from(rng.random::<[u8; 32]>()))?
                            else {
                                continue
                            };
                            let Some((_, second)) = storage.next_dup()? else { continue };
                            let Some((_, third)) = storage.next_dup()? else { continue };
                            let mut slots = [first.key, second.key, third.key];
                            for i in 0..3 {
                                if let Some(slot) = storage.seek_by_key_subkey(
                                    address,
                                    B256::from(rng.random::<[u8; 32]>()),
                                )? && !slots.contains(&slot.key)
                                {
                                    slots[i] = slot.key;
                                }
                            }
                            if let Some((_, account)) = accounts.seek_exact(address)? {
                                entries.push(Entry { address, account, slots });
                            }
                        }
                        Ok(entries)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })?;
        for entry in chunks.into_iter().flatten() {
            if seen.len() == count {
                break
            }
            if !seen.insert(entry.address) {
                continue
            }
            writer.write_all(entry.address.as_slice())?;
            writer.write_all(&entry.account.nonce.to_le_bytes())?;
            writer.write_all(&entry.account.balance.to_be_bytes::<32>())?;
            writer.write_all(entry.account.bytecode_hash.unwrap_or_default().as_slice())?;
            for slot in entry.slots {
                writer.write_all(slot.as_slice())?;
            }
        }
        batch += 1;
        eprintln!("sampled={} elapsed={:.1}", seen.len(), start.elapsed().as_secs_f64());
    }
    writer.flush()?;
    Ok(())
}

fn read_corpus(path: &str) -> Result<Vec<Entry>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut count = [0; 8];
    reader.read_exact(&mut count)?;
    let mut entries = Vec::with_capacity(u64::from_le_bytes(count) as usize);
    for _ in 0..u64::from_le_bytes(count) {
        let mut bytes = [0; 200];
        reader.read_exact(&mut bytes)?;
        let hash = B256::from_slice(&bytes[72..104]);
        entries.push(Entry {
            address: B256::from_slice(&bytes[..32]),
            account: Account {
                nonce: u64::from_le_bytes(bytes[32..40].try_into()?),
                balance: U256::from_be_slice(&bytes[40..72]),
                bytecode_hash: (hash != B256::ZERO).then_some(hash),
            },
            slots: [
                B256::from_slice(&bytes[104..136]),
                B256::from_slice(&bytes[136..168]),
                B256::from_slice(&bytes[168..200]),
            ],
        });
    }
    Ok(entries)
}

fn compute(
    runtime: &Runtime,
    factory: Factory,
    trie: Trie,
    root: B256,
    epoch: u64,
    state: HashedPostState,
) -> Result<(StateRootComputeOutcome, Trie)> {
    let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
    let (_cancel_tx, cancel_rx) = crossbeam_channel::bounded(0);
    let (proof_tx, proof_rx) = crossbeam_channel::unbounded();
    let (hashed_tx, _hashed_rx) = mpsc::channel();
    let workers =
        ProofWorkerHandle::new(runtime, ProofTaskCtx::new(factory), false, proof_tx.clone());
    let mut task = task::SparseTrieCacheTask::new_with_trie(
        runtime,
        updates_rx,
        cancel_rx,
        hashed_tx,
        workers,
        proof_tx,
        proof_rx,
        Default::default(),
        trie,
        root,
        TrieNodeEpoch::new(epoch),
        80,
    );
    updates_tx.send(StateRootMessage::HashedStateUpdate(state))?;
    updates_tx.send(StateRootMessage::FinishedStateUpdates)?;
    drop(updates_tx);
    let outcome = task.run()?;
    let (trie, deferred) = task.into_trie_for_reuse();
    runtime.spawn_drop(deferred);
    Ok((outcome, trie))
}

fn collect_metrics(snapshotter: &Snapshotter, file: &mut impl Write, block: usize) -> Result<()> {
    for (key, _, _, value) in snapshotter.snapshot().into_vec() {
        if let DebugValue::Histogram(mut values) = value &&
            !values.is_empty()
        {
            values.sort_unstable();
            let count = values.len();
            let sum: f64 = values.iter().map(|v| v.0).sum();
            writeln!(
                file,
                "{block},{},{count},{sum},{},{},{},{}",
                key.key().name(),
                values[count / 2],
                values[count * 90 / 100],
                values[count * 99 / 100],
                values[count - 1]
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run(
    runtime: &Runtime,
    snapshotter: &Snapshotter,
    datadir: &str,
    corpus: &str,
    churn: usize,
    blocks: usize,
    warmup: usize,
    mask: u64,
    persistence_threshold: u64,
    cache_gib: usize,
    period: Duration,
    output: &Path,
) -> Result<()> {
    let entries = read_corpus(corpus)?;
    ensure!(entries.len() > HOT + UPDATES, "corpus too small");
    let db = reth_db::open_db(Path::new(datadir).join("db"), DatabaseArguments::default())?;
    let base = {
        let tx = db.tx()?;
        let finish = tx.get::<tables::StageCheckpoints>("Finish".into())?.unwrap();
        ensure!(
            finish
                .finish_stage_checkpoint()
                .and_then(|c| c.partial_state_trie())
                .unwrap_or(finish.block_number) ==
                finish.block_number,
            "recover the snapshot before running"
        );
        finish.block_number
    };
    let rocks = RocksDBProvider::builder(Path::new(datadir).join("rocksdb"))
        .with_default_tables()
        .with_block_cache_size(cache_gib << 30)
        .build()?;
    let factory = Factory::new(
        db,
        MAINNET.clone(),
        StaticFileProvider::read_write(Path::new(datadir).join("static_files"))?,
        rocks,
        runtime.clone(),
    )?
    .with_prune_modes(PruneModes {
        sender_recovery: Some(PruneMode::Full),
        transaction_lookup: Some(PruneMode::Full),
        receipts: Some(PruneMode::Distance(64)),
        ..Default::default()
    });
    let mut parent = factory
        .provider()?
        .header_by_number(base)?
        .ok_or_else(|| eyre::eyre!("missing snapshot header"))?;
    let default_trie = RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
    let mut trie = Trie::default()
        .with_accounts_trie(default_trie.clone())
        .with_default_storage_trie(default_trie)
        .with_state_trie_updates(true);
    let mut root = parent.state_root;
    let mut pending = VecDeque::<ExecutedBlock<EthPrimitives>>::new();
    let (save_tx, save_rx) = mpsc::channel::<SaveBlocksInput<EthPrimitives>>();
    let (done_tx, done_rx) = mpsc::channel();
    let persist_factory = factory.clone();
    let saves_path = output.join("saves.csv");
    let persist = thread::spawn(move || -> Result<()> {
        let mut log = BufWriter::new(File::create(saves_path)?);
        writeln!(log, "db_tip,partial_tip,duration_ms")?;
        for input in save_rx {
            let start = Instant::now();
            let provider = persist_factory.provider_rw()?;
            provider.save_blocks(&input).inspect_err(|e| eprintln!("save_blocks failed: {e:?}"))?;
            provider.commit().inspect_err(|e| eprintln!("commit failed: {e:?}"))?;
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            writeln!(log, "{},{},{elapsed}", input.new_db_tip(), input.new_partial_state_trie())?;
            log.flush()?;
            done_tx.send((input.new_db_tip(), input.new_partial_state_trie()))?;
        }
        Ok(())
    });
    let mut log = BufWriter::new(File::create(output.join("blocks.csv"))?);
    writeln!(log, "block,measured,root_ms,prepare_ms,input_late_ms,cleanup_ms,prune_ms,root_ready_ms,deadline_ms,account_misses,slot_misses,hot_account_evictions,hot_slot_evictions,pending_blocks,partial_lag,retained_storages")?;
    let mut metric_log = BufWriter::new(File::create(output.join("metrics.csv"))?);
    writeln!(metric_log, "block,metric,count,sum,p50,p90,p99,max")?;
    let mut rng = StdRng::seed_from_u64(7012026);
    let mut db_tip = base;
    let mut partial = base;
    let mut saving = false;
    let mut arrival_start = Instant::now();
    let mut cold_position = HOT;
    let mut ever_evicted = B256Set::default();
    let mut evicted_slots = B256Set::default();
    let mut last_epoch = vec![0_u64; entries.len()];
    // Two initialization blocks reveal every hot key. They are not timed as workload blocks.
    for block in 0..blocks + warmup + 2 {
        if block == 2 {
            arrival_start = Instant::now() + period;
        }
        let measured = block >= warmup + 2;
        let arrival = arrival_start + period * block.saturating_sub(2) as u32;
        let start = Instant::now();
        let epoch = base + block as u64 + 1;
        let mut selected = if block < 2 {
            (block * UPDATES..(block + 1) * UPDATES).collect::<Vec<_>>()
        } else {
            sample(&mut rng, HOT, UPDATES * (100 - churn) / 100).into_vec()
        };
        let mut attempts = 0;
        while selected.len() < UPDATES {
            let index = cold_position;
            cold_position += 1;
            if cold_position == entries.len() {
                cold_position = HOT;
            }
            attempts += 1;
            ensure!(
                attempts <= entries.len() - HOT,
                "cold corpus exhausted; enlarge corpus or decrease retention"
            );
            let entry = &entries[index];
            if !trie.is_account_revealed(entry.address) &&
                entry
                    .slots
                    .iter()
                    .all(|slot| !trie.check_valid_storage_witness(entry.address, *slot))
            {
                selected.push(index);
            }
        }
        let mut state = HashedPostState::default();
        let mut account_misses = 0;
        let mut slot_misses = 0;
        for index in selected {
            last_epoch[index] = epoch;
            let entry = &entries[index];
            if !trie.is_account_revealed(entry.address) {
                account_misses += 1;
                if block >= 2 && index < HOT {
                    ever_evicted.insert(entry.address);
                }
            }
            for slot in entry.slots {
                if !trie.check_valid_storage_witness(entry.address, slot) {
                    slot_misses += 1;
                    if block >= 2 && index < HOT {
                        evicted_slots.insert(alloy_primitives::keccak256(
                            [entry.address.as_slice(), slot.as_slice()].concat(),
                        ));
                    }
                }
            }
            let mut account = entry.account;
            account.nonce = account.nonce.checked_add(epoch).unwrap();
            state.accounts.insert(entry.address, Some(account));
            state.storages.insert(
                entry.address,
                HashedStorage {
                    storage: entry
                        .slots
                        .iter()
                        .map(|s| (*s, slot_value(entry.address, *s, epoch)))
                        .collect(),
                },
            );
        }
        ensure!(
            state.accounts.len() == UPDATES && state.storages.len() == UPDATES,
            "duplicate update keys"
        );
        let prepare_ms = start.elapsed().as_secs_f64() * 1000.;
        let input_late_ms = if block >= 2 {
            Instant::now().saturating_duration_since(arrival).as_secs_f64() * 1000.
        } else {
            0.
        };
        if block >= 2 && Instant::now() < arrival {
            thread::sleep(arrival - Instant::now());
        }
        let cleanup_start = Instant::now();
        let mut prune_cutoff = None;
        let completed = match done_rx.try_recv() {
            Ok(done) => Some(done),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                eyre::bail!("persistence worker exited: {:?}", persist.join().unwrap())
            }
        };
        if let Some((new_db, new_partial)) = completed {
            db_tip = new_db;
            partial = new_partial;
            saving = false;
            while pending.front().is_some_and(|b| b.recovered_block().number() <= partial) {
                pending.pop_front();
            }
            prune_cutoff = Some(partial + 1);
        }
        let cleanup_ms = cleanup_start.elapsed().as_secs_f64() * 1000.;
        let root_start = Instant::now();
        let (outcome, next_trie) = compute(runtime, factory.clone(), trie, root, epoch, state)?;
        let root_ms = root_start.elapsed().as_secs_f64() * 1000.;
        let root_ready_ms = if measured { arrival.elapsed().as_secs_f64() * 1000. } else { 0. };
        root = outcome.state_root;
        trie = next_trie;
        let parent_hash = parent.hash_slow();
        parent = Header {
            parent_hash,
            number: epoch,
            state_root: root,
            timestamp: parent.timestamp + 1,
            gas_limit: 1_000_000_000,
            ..Default::default()
        };
        let recovered = RecoveredBlock::new_unhashed(
            Block { header: parent.clone(), body: Default::default() },
            Vec::new(),
        );
        let mut executed =
            ExecutedBlock::new(Arc::new(recovered), Default::default(), Default::default());
        executed.state_trie_updates = outcome.state_trie_updates;
        pending.push_back(executed);
        let mut prune_ms = 0.;
        let mut hot_account_evictions = 0;
        let mut hot_slot_evictions = 0;
        if let Some(cutoff) = prune_cutoff {
            let before: Vec<_> = entries[..HOT]
                .iter()
                .enumerate()
                .filter(|(i, _)| last_epoch[*i] < cutoff || *i % 200 == block % 200)
                .map(|(i, e)| {
                    (
                        i,
                        trie.is_account_revealed(e.address),
                        e.slots.map(|s| trie.check_valid_storage_witness(e.address, s)),
                    )
                })
                .collect();
            let prune_start = Instant::now();
            trie.prune(TrieNodeEpoch::new(cutoff));
            prune_ms = prune_start.elapsed().as_secs_f64() * 1000.;
            for (index, account, slots) in before {
                let entry = &entries[index];
                if account && !trie.is_account_revealed(entry.address) {
                    hot_account_evictions += 1;
                    ever_evicted.insert(entry.address);
                }
                for (i, was) in slots.into_iter().enumerate() {
                    if was && !trie.check_valid_storage_witness(entry.address, entry.slots[i]) {
                        hot_slot_evictions += 1;
                        // Slots are unique within an account; hash the pair to count distinct
                        // evictions.
                        evicted_slots.insert(alloy_primitives::keccak256(
                            [entry.address.as_slice(), entry.slots[i].as_slice()].concat(),
                        ));
                    }
                }
            }
        }
        // Engine threshold scheduling, with a five-block memory buffer and configurable masking.
        if !saving && epoch - db_tip > persistence_threshold {
            let new_db = epoch - 5;
            let new_partial = new_db.saturating_sub(mask).max(partial);
            save_tx.send(SaveBlocksInput::new(
                pending
                    .iter()
                    .filter(|b| b.recovered_block().number() <= new_db)
                    .cloned()
                    .collect(),
                db_tip,
                partial,
                new_db,
                new_partial,
            ))?;
            saving = true;
        }
        let deadline_ms = if measured { arrival.elapsed().as_secs_f64() * 1000. } else { 0. };
        writeln!(log, "{block},{measured},{root_ms},{prepare_ms},{input_late_ms},{cleanup_ms},{prune_ms},{root_ready_ms},{deadline_ms},{account_misses},{slot_misses},{hot_account_evictions},{hot_slot_evictions},{},{},{}", epoch - db_tip, epoch - partial, trie.retained_storage_tries_count())?;
        log.flush()?;
        collect_metrics(snapshotter, &mut metric_log, block)?;
        if block % 10 == 0 {
            eprintln!("block={block} root_ms={root_ms:.1} deadline_ms={deadline_ms:.1} partial_lag={} retained={}", epoch-partial, trie.retained_storage_tries_count());
        }
    }
    let drain = Instant::now();
    if saving {
        (db_tip, partial) = done_rx.recv_timeout(Duration::from_secs(600))?;
        while pending.front().is_some_and(|b| b.recovered_block().number() <= partial) {
            pending.pop_front();
        }
    }
    save_tx.send(SaveBlocksInput::new(
        pending.into_iter().collect(),
        db_tip,
        partial,
        parent.number,
        parent.number,
    ))?;
    done_rx.recv_timeout(Duration::from_secs(600))?;
    let drain_secs = drain.elapsed().as_secs_f64();
    drop(save_tx);
    persist.join().unwrap()?;
    drop(trie);
    let provider = factory.provider()?;
    let mut proof = ProofCalculator::new(provider.state_trie_account_cursor()?);
    let node = proof.root_node()?;
    ensure!(
        proof.compute_root_hash(&[node])? == Some(root),
        "persisted root differs from sparse root"
    );
    let mut check_state = HashedPostState::default();
    let mut accounts = provider.state_trie_account_cursor()?;
    let mut checked = 0;
    for (index, entry) in entries.iter().enumerate() {
        if last_epoch[index] == 0 || (index >= 1024 && index % (entries.len() / 4096).max(1) != 0) {
            continue
        }
        let Some(StateTrieNode::Leaf { value, .. }) =
            accounts.get(Nibbles::unpack(entry.address))?
        else {
            eyre::bail!("missing persisted account")
        };
        let mut expected = entry.account;
        expected.nonce += last_epoch[index];
        ensure!(
            value.nonce == expected.nonce && value.balance == expected.balance,
            "persisted account value mismatch"
        );
        check_state.accounts.insert(entry.address, Some(expected));
        let mut cursor = provider.state_trie_storage_cursor(entry.address)?;
        let mut storage = HashedStorage::default();
        for slot in entry.slots {
            let Some(StateTrieNode::Leaf { value, .. }) = cursor.get(Nibbles::unpack(slot))? else {
                eyre::bail!("missing persisted slot")
            };
            ensure!(
                value == slot_value(entry.address, slot, last_epoch[index]),
                "persisted slot value mismatch"
            );
            storage.storage.insert(slot, value);
        }
        check_state.storages.insert(entry.address, storage);
        checked += 1;
    }
    let empty = RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
    let fresh = Trie::default()
        .with_accounts_trie(empty.clone())
        .with_default_storage_trie(empty)
        .with_state_trie_updates(true);
    let (check, _) =
        compute(runtime, factory.clone(), fresh, root, parent.number + 1, check_state)?;
    ensure!(check.state_root == root, "fresh trie disagrees with persisted proofs");
    eprintln!(
        "verified persisted values and fresh proofs for {checked} accounts and {} slots",
        checked * 3
    );
    let mut summary = File::create(output.join("result.txt"))?;
    writeln!(summary, "churn={churn}\nmask={mask}\npersistence_threshold={persistence_threshold}\ncache_gib={cache_gib}\nblocks={blocks}\nwarmup={warmup}\nhot_accounts={HOT}\nhot_slots={}\nunique_hot_accounts_pruned={}\nunique_hot_slots_pruned={}\ndrain_secs={drain_secs}\nfinal_root={root}\nfinal_block={}\npersisted_root_verified=true\nverified_accounts={checked}\nfresh_trie_verified=true", HOT*3, ever_evicted.len(), evicted_slots.len(), parent.number)?;
    ensure!(ever_evicted.len() * 1000 < HOT, "premature hot account pruning exceeded 0.1%");
    ensure!(evicted_slots.len() * 1000 < HOT * 3, "premature hot slot pruning exceeded 0.1%");
    Ok(())
}

/// Vary full-width values independently across accounts, slots, and blocks.
fn slot_value(address: B256, slot: B256, epoch: u64) -> U256 {
    let mut input = [0; 72];
    input[..32].copy_from_slice(address.as_slice());
    input[32..64].copy_from_slice(slot.as_slice());
    input[64..].copy_from_slice(&epoch.to_be_bytes());
    U256::from_be_bytes(alloy_primitives::keccak256(input).0)
}
