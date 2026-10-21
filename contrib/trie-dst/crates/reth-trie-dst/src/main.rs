//! Bounded workload for the sparse-trie, proof, overlay and persistence pipeline.

mod model;

use alloy_consensus::Header;
use alloy_primitives::{keccak256, Address, B256, U256};
use eyre::{ensure, Context, Result};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use model::{apply, generate, root, storage_root, Model, Operation};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_db::{init_db, DatabaseEnv};
use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_engine_primitives::MIN_PERSISTENCE_BACKPRESSURE_THRESHOLD;
use reth_engine_tree::tree::{
    state_root_strategy::{
        publish_sparse_trie, DefaultStateRootStrategy, StateAccessHint, StateRootTaskOptions,
    },
    TreeConfig, CHANGESET_CACHE_RETENTION_BLOCKS,
};
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult};
use reth_node_builder::common::{LaunchContext, WithConfigs};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives_traits::{AlloyBlockHeader, RecoveredBlock, SealedHeader};
use reth_provider::{
    providers::LatestStateProviderRef, test_utils::MockNodeTypesWithDB, AccountReader,
    DatabaseProviderROFactory, HeaderProvider, ProviderFactory, SaveBlocksInput,
    StageCheckpointReader, StateProofProvider, StateProvider, StateRootProvider,
};
use reth_stages_types::StageId;
use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
use reth_tasks::{RayonConfig, RuntimeBuilder, RuntimeConfig, TokioConfig};
use reth_trie::{HashedPostState, SortedTrieData};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{mpsc, Arc},
    time::Duration,
};

type Node = MockNodeTypesWithDB<Arc<DatabaseEnv>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct JournalBlock {
    number: u64,
    operations: Vec<Operation>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    seed: u64,
    datadir: Option<PathBuf>,
    blocks: u64,
    updates: usize,
    proof_workers: usize,
    timeout_secs: u64,
    persistence_threshold: u64,
    num_state_masking_blocks: u64,
    memory_block_buffer_target: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        let tree = TreeConfig::default();
        Self {
            seed: 0,
            datadir: None,
            blocks: 64,
            updates: 64,
            proof_workers: 2,
            timeout_secs: 60,
            persistence_threshold: tree.persistence_threshold(),
            num_state_masking_blocks: tree.num_state_masking_blocks(),
            memory_block_buffer_target: None,
        }
    }
}

fn verify(factory: &ProviderFactory<Node>, state: &Model, tip: u64, state_tip: u64) -> Result<()> {
    let provider = factory.provider()?;
    let checkpoint = provider
        .get_stage_checkpoint(StageId::Finish)?
        .ok_or_else(|| eyre::eyre!("missing Finish checkpoint"))?;
    ensure!(checkpoint.block_number == tip, "Finish != {tip}");
    let actual_state_tip = checkpoint
        .finish_stage_checkpoint()
        .and_then(|finish| finish.partial_state_trie())
        .unwrap_or(checkpoint.block_number);
    ensure!(actual_state_tip == state_tip, "state/trie frontier != {state_tip}");
    let hash =
        provider.sealed_header(tip)?.ok_or_else(|| eyre::eyre!("missing header {tip}"))?.hash();
    drop(provider);
    let view = OverlayStateProviderFactory::new(
        factory.clone(),
        factory.overlay_manager().overlay_builder(hash),
    )
    .database_provider_ro()?;
    for (address, entry) in state {
        ensure!(
            view.basic_account(address)?.as_ref() == Some(&entry.account),
            "plain account {address} at {tip}"
        );
        // Include deleted and never-written slots, not only surviving leaves.
        let slots = (0..64).map(|slot| B256::from(U256::from(slot))).collect::<Vec<_>>();
        for slot in &slots {
            ensure!(
                view.storage(*address, *slot)?.unwrap_or_default() ==
                    entry.storage.get(&U256::from_be_bytes(slot.0)).copied().unwrap_or_default(),
                "plain storage {address}/{slot} at {tip}"
            );
        }
        let proof = view.proof(Default::default(), *address, &slots)?;
        ensure!(proof.info.as_ref() == Some(&entry.account), "proof account {address} at {tip}");
        ensure!(proof.storage_root == storage_root(entry), "proof storage root {address} at {tip}");
        proof.verify(root(state))?;
    }
    drop(view);
    if tip == state_tip {
        let provider = factory.provider()?;
        let accounts = provider
            .tx_ref()
            .cursor_read::<tables::HashedAccounts>()?
            .walk(None)?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let expected = state
            .iter()
            .map(|(address, entry)| (keccak256(address), entry.account.to_owned()))
            .collect::<BTreeMap<_, _>>();
        ensure!(accounts == expected, "persisted hashed accounts at {tip}");
        let storage = provider
            .tx_ref()
            .cursor_read::<tables::HashedStorages>()?
            .walk(None)?
            .map(|row| row.map(|(address, entry)| ((address, entry.key), entry.value)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let expected = state
            .iter()
            .flat_map(|(address, entry)| {
                entry.storage.iter().map(move |(slot, value)| {
                    ((keccak256(address), keccak256(B256::from(*slot))), *value)
                })
            })
            .collect::<BTreeMap<_, _>>();
        ensure!(storage == expected, "persisted hashed storage at {tip}");
        let latest = LatestStateProviderRef::new(&provider);
        ensure!(
            latest.state_root(HashedPostState::default())? == root(state),
            "persisted root at {tip}"
        );
    }
    Ok(())
}

fn metric_counts(snapshotter: &Snapshotter) -> BTreeMap<String, u64> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .filter_map(|(key, _, _, value)| {
            let name = key.key().name().to_string();
            if !(name.contains("tree.root") ||
                name.contains("state_trie_overlay") ||
                name.contains("storage.overlay"))
            {
                return None;
            }
            let count = match value {
                DebugValue::Counter(count) => count,
                DebugValue::Histogram(values) => values.len() as u64,
                _ => return None,
            };
            Some((name, count))
        })
        .collect()
}

fn run(config: &Config, snapshotter: &Snapshotter) -> Result<()> {
    ensure!((1..=1024).contains(&config.blocks), "blocks must be 1..=1024");
    ensure!((1..=4096).contains(&config.updates), "updates must be 1..=4096");
    ensure!((1..=16).contains(&config.proof_workers), "proof_workers must be 1..=16");
    ensure!((1..=3600).contains(&config.timeout_secs), "timeout_secs must be 1..=3600");
    let buffer = config.memory_block_buffer_target.unwrap_or_else(|| {
        TreeConfig::default().memory_block_buffer_target().min(config.persistence_threshold)
    });
    let masking =
        if config.persistence_threshold == 0 { 0 } else { config.num_state_masking_blocks };
    let backpressure =
        config.persistence_threshold.saturating_mul(2).max(MIN_PERSISTENCE_BACKPRESSURE_THRESHOLD);
    ensure!(backpressure > config.persistence_threshold, "persistence threshold is too large");
    ensure!(buffer <= config.persistence_threshold, "buffer must not exceed persistence threshold");
    ensure!(
        masking == 0 ||
            matches!(masking.checked_add(buffer), Some(window) if window < config.persistence_threshold),
        "masking blocks + buffer must be less than persistence threshold"
    );
    // Clear the default window before applying smaller thresholds: TreeConfig checks each setter.
    let tree_config = TreeConfig::default()
        .with_num_state_masking_blocks(0)
        .with_memory_block_buffer_target(0)
        .with_persistence_backpressure_threshold(u64::MAX)
        .with_persistence_threshold(config.persistence_threshold)
        .with_num_state_masking_blocks(masking)
        .with_memory_block_buffer_target(buffer)
        .with_persistence_backpressure_threshold(backpressure);
    let runtime = RuntimeBuilder::new(RuntimeConfig {
        tokio: TokioConfig::with_worker_threads(2),
        rayon: RayonConfig {
            cpu_threads: Some(2),
            rpc_threads: Some(1),
            storage_threads: Some(2),
            proof_storage_worker_threads: Some(config.proof_workers),
            proof_account_worker_threads: Some(config.proof_workers),
            prewarming_threads: Some(1),
            bal_streaming_threads: Some(1),
            state_trie_overlay_worker_threads: Some(1),
            ..Default::default()
        },
    })
    .build()?;
    // A supplied directory survives process death; standalone runs default to a temporary one.
    let temporary = config.datadir.is_none().then(tempfile::tempdir).transpose()?;
    let datadir = config.datadir.as_deref().unwrap_or_else(|| temporary.as_ref().unwrap().path());
    fs::create_dir_all(datadir)?;
    let journal_path = datadir.join("blocks.jsonl");
    let mut journal = BTreeMap::<u64, JournalBlock>::new();
    if journal_path.exists() {
        for line in
            fs::read(&journal_path)?.split(|byte| *byte == b'\n').filter(|line| !line.is_empty())
        {
            let block: JournalBlock = match serde_json::from_slice(line) {
                Ok(block) => block,
                // SIGKILL may interrupt the final append. Each new append starts on a new line.
                Err(error) if error.is_eof() => continue,
                Err(error) => return Err(error.into()),
            };
            ensure!((1..=config.blocks).contains(&block.number), "journal block out of range");
            if let Some(previous) = journal.insert(block.number, block.clone()) {
                ensure!(previous == block, "conflicting journal block {}", block.number);
            }
        }
    }
    let mut journal_file = OpenOptions::new().create(true).append(true).open(&journal_path)?;
    let chain = Arc::new(ChainSpec::default());
    let data_dir = ChainPath::<DataDirPath>::new(
        datadir.to_string_lossy().parse()?,
        chain.chain,
        Default::default(),
    );
    let db = Arc::new(init_db(data_dir.db(), Default::default())?);
    let context = LaunchContext::new(runtime.clone(), data_dir)
        .with(WithConfigs { config: NodeConfig::new(chain), toml_config: Default::default() })
        .attach(db);
    println!("{}", json!({"recovery": "start"}));
    // This is the node's startup path, including consistency checks, the durable unwind
    // marker and Merkle's walk_all_changed_branch_children recovery mode.
    let factory = runtime.handle().block_on(
        context.create_provider_factory_with_recovery::<Node, EthEvmConfig>(
            OverlayManager::new(runtime.state_trie_overlay_worker_pool()),
            None,
            &[],
            |factory| {
                let checkpoint =
                    factory.provider()?.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default();
                let partial = checkpoint
                    .finish_stage_checkpoint()
                    .and_then(|finish| finish.partial_state_trie());
                println!(
                    "{}",
                    json!({"recovery": "frontiers", "finish": checkpoint.block_number,
                    "partial_state_trie": partial})
                );
                Ok(())
            },
        ),
    )?;
    reth_db_common::init::init_genesis(&factory)?;
    let manager = factory.overlay_manager();
    let base =
        factory.provider()?.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default().block_number;
    let mut parent = factory
        .provider()?
        .sealed_header(base)?
        .ok_or_else(|| eyre::eyre!("missing header {base}"))?;
    let base_hash = parent.hash();
    let strategy = DefaultStateRootStrategy::default();
    let mut state = Model::default();
    for number in 1..=base {
        let block =
            journal.get(&number).ok_or_else(|| eyre::eyre!("missing journal block {number}"))?;
        apply(&mut state, number, &block.operations);
    }
    if base > 0 {
        verify(&factory, &state, base, base)?;
    }
    ensure!(root(&state) == parent.state_root(), "recovered root at {base}");
    println!("{}", json!({"recovery": "complete", "head": base, "root": root(&state)}));
    let mut blocks = Vec::<ExecutedBlock<EthPrimitives>>::new();
    let mut snapshots = vec![state.clone()];
    let timeout = Duration::from_secs(config.timeout_secs);
    let (request_tx, request_rx) = mpsc::sync_channel::<SaveBlocksInput>(1);
    let (ack_tx, ack_rx) = mpsc::sync_channel(1);
    let writer_factory = factory.clone();
    let writer = std::thread::spawn(move || {
        while let Ok(input) = request_rx.recv() {
            let result = (|| -> Result<()> {
                let provider = writer_factory.provider_rw()?;
                provider.save_blocks(&input)?;
                provider.commit()?;
                Ok(())
            })();
            if ack_tx.send(result).is_err() {
                break;
            }
        }
    });
    let mut db_tip = base;
    let mut state_tip = base;
    let mut commits = 0;
    let mut partial_commits = 0;
    let mut update_batches = 0;
    let mut hint_batches = 0;
    let mut pending_save = None;
    let mut prune_requested = false;
    let mut shutdown = false;
    loop {
        let head = base + blocks.len() as u64;
        if let Some((next_db, next_state)) = pending_save {
            let result = if shutdown ||
                head.saturating_sub(state_tip).saturating_sub(buffer) >= backpressure
            {
                Some(ack_rx.recv_timeout(timeout)?)
            } else {
                match ack_rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        eyre::bail!("persistence worker disconnected")
                    }
                }
            };
            if let Some(result) = result {
                result?;
                pending_save = None;
                db_tip = next_db;
                state_tip = next_state;
                commits += 1;
                partial_commits += u64::from(db_tip != state_tip);
                println!(
                    "{}",
                    json!({"persistence": "complete", "head": head,
                    "db_tip": db_tip, "state_tip": state_tip})
                );
                manager.evict_cached_changesets(
                    db_tip.saturating_sub(CHANGESET_CACHE_RETENTION_BLOCKS),
                );
                verify(&factory, &snapshots[(db_tip - base) as usize], db_tip, state_tip)?;
                manager.remove_blocks(
                    blocks
                        .iter()
                        .filter(|block| block.recovered_block().number() <= state_tip)
                        .map(|block| block.recovered_block().hash()),
                );
                let anchor = if state_tip == base {
                    base_hash
                } else {
                    blocks[(state_tip - base) as usize - 1].recovered_block().hash()
                };
                manager.precompute_execution_overlay(parent.hash(), anchor);
                prune_requested = true;
                continue;
            }
        }
        if pending_save.is_none() {
            // Match EngineApiTreeHandler::get_save_blocks_input's Threshold and Head targets.
            let next_db = if shutdown { head } else { head.saturating_sub(buffer) };
            if shutdown ||
                (head.saturating_sub(state_tip) > config.persistence_threshold &&
                    next_db > db_tip)
            {
                let next_state =
                    if shutdown { head } else { next_db.saturating_sub(masking).max(state_tip) };
                if next_db != db_tip || next_state != state_tip {
                    println!(
                        "{}",
                        json!({"persistence": "start", "head": head,
                        "db_tip": db_tip, "state_tip": state_tip,
                        "next_db_tip": next_db, "next_state_tip": next_state,
                        "shutdown": shutdown})
                    );
                    request_tx.send(SaveBlocksInput::new(
                        blocks[(state_tip - base) as usize..(next_db - base) as usize].to_vec(),
                        db_tip,
                        state_tip,
                        next_db,
                        next_state,
                    ))?;
                    pending_save = Some((next_db, next_state));
                    continue;
                }
            }
        }
        if head == config.blocks {
            if shutdown {
                break;
            }
            // Let the last block trigger normal threshold persistence before shutdown.
            shutdown = true;
            continue;
        }
        let number = head + 1;
        let operations = if let Some(block) = journal.get(&number) {
            block.operations.clone()
        } else {
            // New input is journaled before it can reach persistence. Replayed input is read,
            // never regenerated, so a completed commit does not depend on its acknowledgement.
            let mut rng = StdRng::seed_from_u64(config.seed.wrapping_add(number));
            let operations = generate(&mut rng, &state, config.updates);
            let record =
                serde_json::to_vec(&JournalBlock { number, operations: operations.clone() })?;
            journal_file.write_all(b"\n")?;
            journal_file.write_all(&record)?;
            journal_file.write_all(b"\n")?;
            operations
        };
        let (bundle, updates) = apply(&mut state, number, &operations);
        // Print before entering production tasks so a panic still leaves the input on disk.
        println!(
            "{}",
            json!({"block": number, "seed": config.seed, "db_tip": db_tip,
            "state_tip": state_tip, "operations": operations})
        );
        let factory_view = OverlayStateProviderFactory::new(
            factory.clone(),
            manager.overlay_builder(parent.hash()),
        );
        let retained = std::mem::take(&mut prune_requested).then(|| {
            blocks
                .iter()
                .filter(|block| block.recovered_block().number() > state_tip)
                .rev()
                .cloned()
                .collect()
        });
        let (mut handle, pending) = strategy.spawn_state_root(
            &runtime,
            factory_view,
            StateRootTaskOptions {
                parent_header: parent.clone(),
                preserved_sparse_trie: manager.take_sparse_trie(),
                transaction_count: None,
                gas_used: None,
                config: &tree_config,
                pending_sparse_trie_prune_blocks: retained,
            },
        );
        let stream = handle.take_hashed_update_stream();
        let hints = handle.take_hint_stream();
        let mut rng = StdRng::seed_from_u64(config.seed.wrapping_add(number).rotate_left(32));
        let written = updates
            .iter()
            .flat_map(|update| {
                update
                    .accounts
                    .keys()
                    .chain(update.storages.keys())
                    .map(|address| (*address, None))
                    .chain(update.storages.iter().flat_map(|(address, storage)| {
                        storage.storage.keys().map(move |slot| (*address, Some(*slot)))
                    }))
            })
            .collect::<BTreeSet<_>>();
        let mut targets = written.iter().copied().collect::<Vec<_>>();
        targets.shuffle(&mut rng);
        // Prewarming sees only some writes; synthetic read-only accesses add false positives.
        let count =
            if targets.len() > 1 { rng.random_range(1..targets.len()) } else { targets.len() };
        targets.truncate(count);
        for _ in 0..rng.random_range(1..=8) {
            let address = Address::from_word(B256::from(U256::from(rng.random_range(17..=32u64))));
            let slot = rng
                .random_bool(0.5)
                .then(|| keccak256(B256::from(U256::from(rng.random_range(0..64u64)))));
            targets.push((keccak256(address), slot));
        }
        let hinted = targets
            .iter()
            .flat_map(|(address, slot)| [(*address, None), (*address, *slot)])
            .collect::<BTreeSet<_>>();
        println!(
            "{}",
            json!({"block_number": number, "hint_coverage": {
            "written": written.len(), "overlap": written.intersection(&hinted).count(),
            "unhinted": written.difference(&hinted).count(),
            "read_only": hinted.difference(&written).count()}})
        );
        targets.shuffle(&mut rng);
        let mut targets = targets.into_iter();
        let mut updates = updates.into_iter();
        while targets.len() > 0 || updates.len() > 0 {
            if targets.len() > 0 && (updates.len() == 0 || rng.random_bool(0.5)) {
                let size = rng.random_range(1..=targets.len().min(16));
                let mut hint = StateAccessHint::default();
                for (address, slot) in targets.by_ref().take(size) {
                    if let Some(slot) = slot {
                        hint.storages.entry(address).or_default().push(slot);
                    } else {
                        hint.accounts.push(address);
                    }
                }
                println!(
                    "{}",
                    json!({"block_number": number, "batch": "hint",
                    "accounts": hint.accounts, "storages": hint.storages})
                );
                hints.on_access_hint(hint);
                hint_batches += 1;
            } else {
                let size = rng.random_range(1..=updates.len().min(16));
                let mut batch = HashedPostState::default();
                for update in updates.by_ref().take(size) {
                    batch.extend(update);
                }
                println!(
                    "{}",
                    json!({"block_number": number, "batch": "update", "operations": size})
                );
                stream.on_hashed_state_update(batch);
                update_batches += 1;
            }
        }
        drop(hints);
        stream.finish();
        let outcome = handle.take_state_root_rx().recv_timeout(timeout)??;
        ensure!(
            outcome.state_root == root(&state),
            "seed {} block {number}: expected {}, actual {}",
            config.seed,
            root(&state),
            outcome.state_root
        );
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: Header {
                    parent_hash: parent.hash(),
                    number,
                    state_root: outcome.state_root,
                    timestamp: number,
                    ..Default::default()
                },
                ..Default::default()
            },
            Vec::new(),
        );
        parent = SealedHeader::new_unhashed(block.header().clone());
        let executed = ExecutedBlock::new(
            Arc::new(block),
            Arc::new(BlockExecutionOutput {
                state: bundle,
                result: BlockExecutionResult {
                    receipts: Vec::new(),
                    requests: Default::default(),
                    gas_used: 0,
                    blob_gas_used: 0,
                },
            }),
            SortedTrieData {
                hashed_state: Arc::new(outcome.hashed_state.as_ref().clone().into_sorted()),
                trie_updates: Arc::new(outcome.trie_updates.clone_into_sorted()),
            },
        );
        manager.insert_block(executed.clone());
        publish_sparse_trie(&pending, manager, parent.hash(), outcome.state_root);
        blocks.push(executed);
        snapshots.push(state.clone());
    }
    drop(request_tx);
    writer.join().map_err(|_| eyre::eyre!("persistence worker panicked"))?;
    // Synchronize final trie completion before taking the metrics snapshot.
    if let Some(trie) = manager.take_sparse_trie() {
        ensure!(trie.into_trie_for(parent.hash())?.is_some(), "missing final reusable trie");
    }
    let metrics = metric_counts(snapshotter);
    println!(
        "{}",
        json!({"completed": true, "seed": config.seed, "blocks": config.blocks,
        "commits": commits, "partial_commits": partial_commits,
        "update_batches": update_batches, "hint_batches": hint_batches,
        "metrics": metrics})
    );
    Ok(())
}

fn main() -> Result<()> {
    let config = std::env::args()
        .nth(1)
        .map(|value| serde_json::from_str::<Config>(&value))
        .transpose()?
        .unwrap_or_default();
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install()?;
    run(&config, &snapshotter).wrap_err_with(|| format!("reth-trie-dst {config:?}"))
}
