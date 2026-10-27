//! Bounded workload for the sparse-trie, proof, overlay and persistence pipeline.

mod input;
mod model;

use alloy_consensus::Header;
use alloy_primitives::{keccak256, B256, U256};
use eyre::{ensure, Context, Result};
use input::{Input, Operation};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use model::{apply, root, Model};
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
    StageCheckpointReader, StateProvider, StateRootProvider,
};
use reth_stages_types::StageId;
use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
use reth_tasks::{RayonConfig, RuntimeBuilder, RuntimeConfig, TokioConfig};
use reth_trie::{HashedPostState, SortedTrieData};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::{fs::MetadataExt, net::UnixDatagram, process::CommandExt},
    path::PathBuf,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

type Node = MockNodeTypesWithDB<Arc<DatabaseEnv>>;

#[cfg(feature = "inline-coverage")]
unsafe extern "C" {
    fn bedrock_coverage_flush();
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct JournalBlock {
    number: u64,
    operations: Vec<Operation>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    seed: u64,
    input: Option<Input>,
    expected_roots: Option<Vec<B256>>,
    /// An atomic mailbox for snapshot continuations, applied before the next new block.
    input_file: Option<PathBuf>,
    datadir: Option<PathBuf>,
    /// Guest-local initialization barrier, used only by the first warm process.
    init_socket: Option<PathBuf>,
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
            input: None,
            expected_roots: None,
            input_file: None,
            datadir: None,
            init_socket: None,
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

#[allow(clippy::clone_on_copy, reason = "Account is not Copy with account-ext enabled.")]
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
        // Include deleted slots and both range endpoints, not only surviving leaves.
        for slot in entry
            .touched_slots
            .iter()
            .copied()
            .chain([U256::ZERO, U256::from(u32::MAX)])
            .map(B256::from)
            .collect::<BTreeSet<_>>()
        {
            ensure!(
                view.storage(*address, slot)?.unwrap_or_default() ==
                    entry.storage.get(&U256::from_be_bytes(slot.0)).copied().unwrap_or_default(),
                "plain storage {address}/{slot} at {tip}"
            );
        }
    }
    drop(view);
    if tip == state_tip {
        let expected_root = root(state);
        let provider = factory.provider()?;
        let accounts = provider
            .tx_ref()
            .cursor_read::<tables::HashedAccounts>()?
            .walk(None)?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let expected = state
            .iter()
            .map(|(address, entry)| (keccak256(address), entry.account.clone()))
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
            latest.state_root(HashedPostState::default())? == expected_root,
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

fn run(config: &mut Config, snapshotter: &Snapshotter) -> Result<()> {
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
    let datadir =
        config.datadir.clone().unwrap_or_else(|| temporary.as_ref().unwrap().path().to_path_buf());
    fs::create_dir_all(&datadir)?;
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
    if let Some(path) = config.init_socket.take() {
        ensure!(
            factory
                .provider()?
                .get_stage_checkpoint(StageId::Finish)?
                .unwrap_or_default()
                .block_number ==
                0,
            "warm initialization requires empty state"
        );
        wait_for_execution(config, &path)?;
    }
    let input = config
        .input
        .take()
        .unwrap_or_else(|| Input::generate(config.seed, config.blocks, config.updates));
    input.validate()?;
    let input_blocks =
        input.operations.split_inclusive(|op| matches!(op, Operation::Commit)).collect::<Vec<_>>();
    let mut total_blocks = input_blocks.len() as u64;
    ensure!(
        config.expected_roots.as_ref().is_none_or(|roots| roots.len() == input_blocks.len()),
        "expected root count does not match input"
    );
    let overrides_path = datadir.join("input-overrides.json");
    #[derive(Default, Serialize, Deserialize)]
    struct Overrides {
        applied: u64,
        blocks: BTreeMap<u64, Vec<Operation>>,
        total_blocks: u64,
    }
    #[derive(Deserialize)]
    struct Injection {
        id: u64,
        input: Input,
    }
    let mut overrides: Overrides = if overrides_path.exists() {
        serde_json::from_slice(&fs::read(&overrides_path)?)?
    } else {
        Overrides::default()
    };
    if overrides.total_blocks != 0 {
        total_blocks = overrides.total_blocks;
        // Saved continuations no longer match roots precomputed for the original input.
        config.expected_roots = None;
        println!("{}", json!({"input": "restored", "blocks": total_blocks}));
    }
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
            ensure!((1..=total_blocks).contains(&block.number), "journal block out of range");
            if let Some(previous) = journal.get(&block.number) {
                ensure!(previous == &block, "conflicting journal block {}", block.number);
            } else {
                journal.insert(block.number, block);
            }
        }
    }
    let mut journal_file = OpenOptions::new().create(true).append(true).open(&journal_path)?;

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
        for operation in &block.operations {
            if let Operation::UpdateBatch { updates } = operation {
                for update in updates {
                    update.apply_to_model(&mut state);
                }
            }
        }
    }
    if base > 0 {
        verify(&factory, &state, base, base)?;
    }
    let recovered_root = root(&state);
    ensure!(recovered_root == parent.state_root(), "recovered root at {base}");
    println!("{}", json!({"recovery": "complete", "head": base, "root": recovered_root}));
    let mut blocks = Vec::<ExecutedBlock<EthPrimitives>>::new();
    // Replay journal updates instead of retaining a full model for every block.
    let mut persisted_state = state.clone();
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
    let mut waiting = false;
    let mut seen_input_file = None;
    loop {
        let head = base + blocks.len() as u64;
        if head < 1024 &&
            journal.range((head + 1)..).next().is_none() &&
            let Some((path, file_stamp)) = config.input_file.as_ref().and_then(|path| {
                fs::metadata(path).ok().map(|metadata| {
                    (
                        path,
                        (metadata.ino(), metadata.mtime(), metadata.mtime_nsec(), metadata.len()),
                    )
                })
            }) &&
            seen_input_file != Some(file_stamp)
        {
            let injection: Injection = serde_json::from_slice(&fs::read(path)?)?;
            if injection.id != overrides.applied {
                injection.input.validate()?;
                let injected = injection.input.blocks();
                let count = injected.len().min((1024 - head) as usize);
                ensure!(count > 0, "continuation exceeds the 1024-block limit");
                overrides.blocks.retain(|n, _| *n < (head + 1));
                for (i, block) in injected.into_iter().take(count).enumerate() {
                    overrides.blocks.insert((head + 1) + i as u64, block);
                }
                total_blocks = head + count as u64;
                overrides.total_blocks = total_blocks;
                overrides.applied = injection.id;
                config.expected_roots = None;
                shutdown = false;
                waiting = false;
                let tmp = overrides_path.with_extension("tmp");
                fs::write(&tmp, serde_json::to_vec(&overrides)?)?;
                fs::rename(tmp, &overrides_path)?;
                println!("{}", json!({"input": "injected", "block": (head + 1), "blocks": count}));
            }
            seen_input_file = Some(file_stamp);
        }
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
                for number in (db_tip + 1)..=next_db {
                    let block = journal
                        .get(&number)
                        .ok_or_else(|| eyre::eyre!("missing journal block {number}"))?;
                    for operation in &block.operations {
                        if let Operation::UpdateBatch { updates } = operation {
                            for update in updates {
                                update.apply_to_model(&mut persisted_state);
                            }
                        }
                    }
                }
                db_tip = next_db;
                state_tip = next_state;
                commits += 1;
                partial_commits += u64::from(db_tip != state_tip);
                manager.evict_cached_changesets(
                    db_tip.saturating_sub(CHANGESET_CACHE_RETENTION_BLOCKS),
                );
                verify(&factory, &persisted_state, db_tip, state_tip)?;
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
                // The controller may SIGKILL this process after this event.
                #[cfg(feature = "inline-coverage")]
                unsafe {
                    bedrock_coverage_flush();
                }
                println!(
                    "{}",
                    json!({"persistence": "complete", "head": head,
                    "db_tip": db_tip, "state_tip": state_tip})
                );
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
        if head == total_blocks {
            if shutdown {
                if head < 1024 &&
                    let Some(path) = &config.input_file &&
                    !path.with_extension("finish").exists()
                {
                    if !waiting {
                        println!("{}", json!({"input": "waiting", "head": head}));
                        waiting = true;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                break;
            }
            // Let the last block trigger normal threshold persistence before shutdown.
            shutdown = true;
            continue;
        }
        let number = head + 1;
        let block = match journal.entry(number) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let block = overrides
                    .blocks
                    .get(&number)
                    .map_or_else(|| input_blocks[number as usize - 1].to_vec(), Clone::clone);
                let journal_block = JournalBlock { number, operations: block };
                let record = serde_json::to_vec(&journal_block)?;
                journal_file.write_all(b"\n")?;
                journal_file.write_all(&record)?;
                journal_file.write_all(b"\n")?;
                entry.insert(journal_block)
            }
        };
        let operations = block.operations.clone();
        let updates_input = operations
            .iter()
            .flat_map(|op| match op {
                Operation::UpdateBatch { updates } => updates.as_slice(),
                _ => &[],
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut accounts = state.keys().copied().collect::<BTreeSet<_>>();
        for op in &updates_input {
            match op {
                model::Update::Account { address, .. } => {
                    accounts.insert(*address);
                }
                model::Update::Storage { address, .. } => ensure!(
                    accounts.contains(address),
                    "storage before account creation: {address}"
                ),
            }
        }
        let (bundle, updates) = apply(&mut state, number, &updates_input);
        // The full operations are in the journal before production tasks run.
        #[cfg(feature = "inline-coverage")]
        unsafe {
            bedrock_coverage_flush();
        }
        println!(
            "{}",
            json!({"block": number, "seed": config.seed, "db_tip": db_tip,
            "state_tip": state_tip, "operations": operations.len()})
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
        let mut hinted = BTreeSet::new();
        let mut updates = updates.into_iter();
        for operation in &operations {
            match operation {
                Operation::PrewarmBatch { accounts, storages } => {
                    let mut hint = StateAccessHint::default();
                    for address in accounts {
                        let address = keccak256(address);
                        hint.accounts.push(address);
                        hinted.insert((address, None));
                    }
                    for storage in storages {
                        let address = keccak256(storage.address);
                        let slot = keccak256(B256::from(storage.slot));
                        hint.storages.entry(address).or_default().push(slot);
                        hinted.insert((address, Some(slot)));
                    }
                    hints.on_access_hint(hint);
                    hint_batches += 1;
                }
                Operation::UpdateBatch { updates: batch_updates } => {
                    let mut batch = HashedPostState::default();
                    for update in updates.by_ref().take(batch_updates.len()) {
                        batch.extend(update);
                    }
                    stream.on_hashed_state_update(batch);
                    update_batches += 1;
                }
                Operation::Commit => {}
            }
        }
        println!(
            "{}",
            json!({"block_number": number, "hint_coverage": {
            "written": written.len(), "overlap": written.intersection(&hinted).count(),
            "unhinted": written.difference(&hinted).count(),
            "read_only": hinted.difference(&written).count()}})
        );
        drop(hints);
        stream.finish();
        let outcome = handle.take_state_root_rx().recv_timeout(timeout)??;
        drop(handle);
        let expected_root = config
            .expected_roots
            .as_ref()
            .and_then(|roots| roots.get(number as usize - 1))
            .copied()
            .unwrap_or_else(|| root(&state));
        ensure!(
            outcome.state_root == expected_root,
            "seed {} block {number}: expected {}, actual {}",
            config.seed,
            expected_root,
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
                hashed_state: Arc::new(match Arc::try_unwrap(outcome.hashed_state) {
                    Ok(state) => state.into_sorted(),
                    Err(state) => state.clone_into_sorted(),
                }),
                trie_updates: Arc::new(match Arc::try_unwrap(outcome.trie_updates) {
                    Ok(updates) => updates.into_sorted(),
                    Err(updates) => updates.clone_into_sorted(),
                }),
            },
        );
        manager.insert_block(executed.clone());
        publish_sparse_trie(&pending, manager, parent.hash(), outcome.state_root);
        blocks.push(executed);
    }
    drop(request_tx);
    writer.join().map_err(|_| eyre::eyre!("persistence worker panicked"))?;
    // Synchronize final trie completion before taking the metrics snapshot.
    if let Some(trie) = manager.take_sparse_trie() {
        ensure!(trie.into_trie_for(parent.hash())?.is_some(), "missing final reusable trie");
    }
    // Root delivery precedes worker teardown. Drain their queues so the last database handle
    // closes before process exit destroys RocksDB's global scheduler.
    for name in ["sparse-trie", "trie-hashing", "storage-workers", "account-workers", "drop"] {
        runtime.spawn_blocking_named(name, || {}).get();
    }
    let metrics = metric_counts(snapshotter);
    println!(
        "{}",
        json!({"completed": true, "seed": config.seed, "blocks": total_blocks,
        "commits": commits, "partial_commits": partial_commits,
        "update_batches": update_batches, "hint_batches": hint_batches,
        "metrics": metrics})
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut argv = std::env::args().skip(1);
    let command = argv.next();
    let text = match command.as_deref() {
        Some("describe") => {
            println!(
                "{}",
                json!({"protocol": "rift-fuzz", "version": 1,
                "capabilities": ["seed-inputs", "mutation-operators", "mutation-sites",
                                 "mutate-input", "validate-input", "reduce-input"]})
            );
            return Ok(());
        }
        Some("reduce-input") => {
            let mut request = Vec::new();
            std::io::stdin().take((32 << 20) + 1).read_to_end(&mut request)?;
            ensure!(request.len() <= 32 << 20, "reduction request exceeds 32 MiB");
            let value: serde_json::Value = serde_json::from_slice(&request)?;
            let text = value["input"]
                .as_str()
                .ok_or_else(|| eyre::eyre!("input must be a JSON string"))?;
            let input: Input = serde_json::from_str(text)?;
            let limit = match value.get("limit") {
                None => 64,
                Some(limit) => limit
                    .as_u64()
                    .filter(|limit| *limit > 0)
                    .ok_or_else(|| eyre::eyre!("limit must be a positive integer"))?,
            };
            let candidates = input.reduce(usize::try_from(limit.min(1024))?, text.len())?;
            println!("{}", serde_json::to_string(&candidates)?);
            return Ok(());
        }
        Some("mutation-operators") => {
            println!(
                "{}",
                json!([
                {"name": "account.address", "selector": {"path": ["operations", "*", "updates", "*", "Account", "address"]}},
                {"name": "account.balance", "selector": {"path": ["operations", "*", "updates", "*", "Account", "account", "balance"]}},
                {"name": "account.nonce", "selector": {"path": ["operations", "*", "updates", "*", "Account", "account", "nonce"]}},
                {"name": "account.bytecode_hash", "selector": {"path": ["operations", "*", "updates", "*", "Account", "account", "bytecode_hash"]}},
                {"name": "storage.address", "selector": {"path": ["operations", "*", "updates", "*", "Storage", "address"]}},
                {"name": "storage.slot", "selector": {"path": ["operations", "*", "updates", "*", "Storage", "slot"]}},
                {"name": "storage.value", "selector": {"path": ["operations", "*", "updates", "*", "Storage", "value"]}},
                {"name": "prewarm.account", "selector": {"path": ["operations", "*", "accounts", "*"]}},
                {"name": "prewarm.storage.address", "selector": {"path": ["operations", "*", "storages", "*", "address"]}},
                {"name": "prewarm.storage.slot", "selector": {"path": ["operations", "*", "storages", "*", "slot"]}},
                {"name": "update.duplicate", "selector": {"path": ["operations", "*", "updates", "*"]}},
                {"name": "update.remove", "selector": {"path": ["operations", "*", "updates", "*"]}},
                {"name": "update.crossover", "donor": true, "selector": {"path": ["operations", "*", "updates", "*"]}},
                {"name": "batch.duplicate", "selector": {"path": ["operations", "*"], "kinds": ["update_batch", "prewarm_batch"]}},
                {"name": "batch.remove", "selector": {"path": ["operations", "*"], "kinds": ["update_batch", "prewarm_batch"]}},
                {"name": "batch.crossover", "donor": true, "selector": {"path": ["operations", "*"], "kinds": ["update_batch", "prewarm_batch"]}},
                {"name": "commit.insert", "selector": {"path": ["operations", "*"], "kinds": ["update_batch"]}},
                {"name": "commit.remove", "selector": {"path": ["operations", "*"], "kinds": ["commit"], "exclude_last": true}}
                ])
            );
            return Ok(());
        }
        Some("mutation-sites") => {
            let mut request = Vec::new();
            std::io::stdin().read_to_end(&mut request)?;
            let mut value: serde_json::Value = serde_json::from_slice(&request)?;
            let input: Input = match std::mem::take(&mut value["input"]) {
                serde_json::Value::String(text) => serde_json::from_str(&text)?,
                input => serde_json::from_value(input)?,
            };
            input.validate()?;
            let operator = value["operator"].as_str();
            let sites = input.mutation_sites(operator);
            println!("{}", serde_json::to_string(&sites)?);
            return Ok(());
        }
        Some("observe") => {
            let mut request = Vec::new();
            std::io::stdin().read_to_end(&mut request)?;
            let value: serde_json::Value = serde_json::from_slice(&request)?;
            let decisions =
                value.get("decisions").ok_or_else(|| eyre::eyre!("missing decisions"))?;
            let changed = value
                .get("original")
                .is_some_and(|original| !original.is_null() && original != decisions);
            println!(
                "{}",
                json!({
                    "decisions": decisions,
                    "decision_diff": if changed { vec!["recorded decisions differ"] } else { vec![] },
                    "moments": [], "events_origin_ns": null
                })
            );
            return Ok(());
        }
        Some("generate-input" | "generate-suffix" | "mutate-input" | "validate-input") => {
            let mut request = Vec::new();
            std::io::stdin().read_to_end(&mut request)?;
            let mut value: serde_json::Value = serde_json::from_slice(&request)?;
            let contract = value["input"].is_string();
            let input = match command.as_deref() {
                Some("generate-input" | "generate-suffix") => {
                    let config: Config = serde_json::from_value(value)?;
                    ensure!(
                        (1..=1024).contains(&config.blocks) && (1..=4096).contains(&config.updates),
                        "invalid input size"
                    );
                    let mut input = Input::generate(config.seed, config.blocks, config.updates);
                    if command.as_deref() == Some("generate-suffix") {
                        let accounts = input
                            .operations
                            .iter()
                            .filter_map(|op| match op {
                                Operation::UpdateBatch { updates } => Some(updates.iter()),
                                _ => None,
                            })
                            .flatten()
                            .filter_map(|update| match update {
                                model::Update::Account { address, .. } => Some(*address),
                                _ => None,
                            })
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .map(|address| model::Update::Account {
                                address,
                                account: reth_primitives_traits::Account {
                                    nonce: 1,
                                    ..Default::default()
                                },
                            })
                            .collect::<Vec<_>>();
                        let Operation::UpdateBatch { updates } = input
                            .operations
                            .iter_mut()
                            .find(|op| matches!(op, Operation::UpdateBatch { .. }))
                            .expect("generated update batch")
                        else {
                            unreachable!()
                        };
                        updates.splice(0..0, accounts);
                    }
                    input
                }
                _ => {
                    let mut input: Input = match std::mem::take(&mut value["input"]) {
                        serde_json::Value::String(text) => serde_json::from_str(&text)?,
                        input => serde_json::from_value(input)?,
                    };
                    if command.as_deref() == Some("mutate-input") {
                        let seed = value["seed"]
                            .as_u64()
                            .ok_or_else(|| eyre::eyre!("missing mutation seed"))?;
                        if let Some(operator) = value["operator"].as_str().map(str::to_owned) {
                            let path: Vec<String> = serde_json::from_value(value["path"].clone())?;
                            let donor: Option<Input> = match std::mem::take(&mut value["donor"]) {
                                serde_json::Value::String(text) => {
                                    Some(serde_json::from_str(&text)?)
                                }
                                donor => serde_json::from_value(donor)?,
                            };
                            let donor_path: Vec<String> = if let Some(donor) = &donor {
                                donor.validate()?;
                                if let Some(path) = value.get("donor_path") {
                                    serde_json::from_value(path.clone())?
                                } else {
                                    let sites = donor.mutation_sites(Some(&operator));
                                    ensure!(!sites.is_empty(), "donor has no compatible sites");
                                    sites[usize::try_from(seed % u64::try_from(sites.len())?)?]
                                        .path
                                        .clone()
                                }
                            } else {
                                Vec::new()
                            };
                            input.mutate_at(
                                &operator,
                                &path,
                                seed,
                                donor.as_ref().map(|donor| (donor, donor_path.as_slice())),
                            )?;
                        } else {
                            input.mutate(seed)?;
                        }
                    }
                    input
                }
            };
            input.validate()?;
            if command.as_deref() == Some("validate-input") {
                let mut state = Model::default();
                let expected_roots = input
                    .operations
                    .split_inclusive(|op| matches!(op, Operation::Commit))
                    .map(|block| {
                        for op in block {
                            if let Operation::UpdateBatch { updates } = op {
                                for update in updates {
                                    update.apply_to_model(&mut state);
                                }
                            }
                        }
                        root(&state)
                    })
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    json!({"blocks": expected_roots.len(), "expected_roots": expected_roots})
                );
                return Ok(());
            }
            let output = serde_json::to_string(&input)?;
            if command.as_deref() == Some("mutate-input") && contract {
                println!("{}", serde_json::to_string(&output)?);
            } else {
                println!("{output}");
            }
            return Ok(());
        }
        Some(hook @ ("parse-args" | "config" | "seed-inputs" | "suffix-seed")) => {
            let controller = std::env::current_exe()?.with_file_name("controller.py");
            return Err(std::process::Command::new(controller).arg(hook).args(argv).exec().into());
        }
        Some("-") => std::io::read_to_string(std::io::stdin())?,
        Some(path) if path.starts_with('@') => fs::read_to_string(&path[1..])?,
        Some(text) => text.to_string(),
        None => "{}".into(),
    };
    let mut config: Config = serde_json::from_str(&text)?;
    drop(text);
    if let Some(input) = &config.input {
        input.validate()?;
    }
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install()?;
    run(&mut config, &snapshotter)
        .wrap_err_with(|| format!("reth-trie-dst seed={} blocks={}", config.seed, config.blocks))
}

// Only these settings affect the runtime, tree configuration and storage initialized above.
fn validate_initialization(initial: &Config, execution: &Config) -> Result<()> {
    ensure!(
        initial.proof_workers == execution.proof_workers &&
            initial.persistence_threshold == execution.persistence_threshold &&
            initial.num_state_masking_blocks == execution.num_state_masking_blocks &&
            initial.memory_block_buffer_target == execution.memory_block_buffer_target &&
            initial.datadir == execution.datadir &&
            execution.init_socket.is_none(),
        "execution changes initialized configuration"
    );
    ensure!((1..=1024).contains(&execution.blocks), "blocks must be 1..=1024");
    ensure!((1..=4096).contains(&execution.updates), "updates must be 1..=4096");
    ensure!((1..=3600).contains(&execution.timeout_secs), "timeout_secs must be 1..=3600");
    Ok(())
}

fn wait_for_execution(config: &mut Config, path: &std::path::Path) -> Result<()> {
    let socket = UnixDatagram::bind(path)?;
    let deadline = Instant::now() + Duration::from_secs(3600);
    let mut message = [0u8; 4096];
    loop {
        socket.set_read_timeout(Some(
            deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| eyre::eyre!("initialization barrier timed out"))?,
        ))?;
        let (size, peer) = socket.recv_from(&mut message)?;
        let peer = peer.as_pathname().ok_or_else(|| eyre::eyre!("unnamed barrier peer"))?;
        if &message[..size] == b"ready" {
            socket.send_to(b"initialized", peer)?;
            continue;
        }
        let execution: Config =
            serde_json::from_slice(&fs::read(std::str::from_utf8(&message[..size])?)?)?;
        validate_initialization(config, &execution)?;
        *config = execution;
        socket.send_to(b"started", peer)?;
        fs::remove_file(path)?;
        return Ok(());
    }
}
