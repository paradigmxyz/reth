//! Bounded workload for the sparse-trie, proof, overlay and persistence pipeline.

mod model;

use alloy_consensus::Header;
use alloy_primitives::{keccak256, B256, U256};
use eyre::{ensure, Context, Result};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use model::{generate, root, storage_root, Model};
use rand::{rngs::StdRng, SeedableRng};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_engine_primitives::MIN_PERSISTENCE_BACKPRESSURE_THRESHOLD;
use reth_engine_tree::tree::{
    state_root_strategy::{publish_sparse_trie, DefaultStateRootStrategy, StateRootTaskOptions},
    TreeConfig, CHANGESET_CACHE_RETENTION_BLOCKS,
};
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult};
use reth_primitives_traits::{AlloyBlockHeader, RecoveredBlock, SealedHeader};
use reth_provider::{
    providers::LatestStateProviderRef,
    test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
    AccountReader, DatabaseProviderROFactory, HeaderProvider, ProviderFactory, SaveBlocksInput,
    StageCheckpointReader, StateProofProvider, StateProvider, StateRootProvider,
};
use reth_stages_types::StageId;
use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
use reth_tasks::{RayonConfig, RuntimeBuilder, RuntimeConfig, TokioConfig};
use reth_trie::{HashedPostState, SortedTrieData};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{mpsc, Arc},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    seed: u64,
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

fn verify(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    state: &Model,
    tip: u64,
    state_tip: u64,
) -> Result<()> {
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
    let factory = create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()))
        .with_overlay_manager(OverlayManager::new(runtime.state_trie_overlay_worker_pool()));
    reth_db_common::init::init_genesis(&factory)?;
    let manager = factory.overlay_manager();
    let mut parent =
        factory.provider()?.sealed_header(0)?.ok_or_else(|| eyre::eyre!("missing genesis"))?;
    let strategy = DefaultStateRootStrategy::default();
    let genesis_hash = parent.hash();
    let mut rng = StdRng::seed_from_u64(config.seed);
    let mut state = Model::default();
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
    let mut db_tip = 0;
    let mut state_tip = 0;
    let mut commits = 0;
    let mut partial_commits = 0;
    let mut pending_save = None;
    let mut prune_requested = false;
    let mut shutdown = false;
    loop {
        let head = blocks.len() as u64;
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
                verify(&factory, &snapshots[db_tip as usize], db_tip, state_tip)?;
                manager.remove_blocks(
                    blocks
                        .iter()
                        .filter(|block| block.recovered_block().number() <= state_tip)
                        .map(|block| block.recovered_block().hash()),
                );
                let anchor = if state_tip == 0 {
                    genesis_hash
                } else {
                    blocks[state_tip as usize - 1].recovered_block().hash()
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
                        blocks[state_tip as usize..next_db as usize].to_vec(),
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
        let (bundle, updates, operations) = generate(&mut rng, &mut state, number, config.updates);
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
        // Only the hashed update stream is used by this workload.
        drop(handle.take_hint_stream());
        for update in updates {
            stream.on_hashed_state_update(update);
        }
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
