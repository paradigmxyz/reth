//! Bounded workload for the sparse-trie, proof, overlay and persistence pipeline.

mod model;

use alloy_consensus::Header;
use alloy_primitives::{keccak256, B256, U256};
use eyre::{ensure, Context, Result};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use model::{generate, root, storage_root, Model};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_engine_tree::tree::{
    state_root_strategy::{publish_sparse_trie, DefaultStateRootStrategy, StateRootTaskOptions},
    TreeConfig,
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
use reth_storage_overlay::OverlayStateProviderFactory;
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
}

impl Default for Config {
    fn default() -> Self {
        Self { seed: 0, blocks: 64, updates: 64, proof_workers: 2, timeout_secs: 60 }
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
            view.basic_account(address)? == Some(entry.account),
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
        ensure!(proof.info == Some(entry.account), "proof account {address} at {tip}");
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
            .map(|(address, entry)| (keccak256(address), entry.account))
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
    ensure!((16..=1024).contains(&config.blocks), "blocks must be 16..=1024");
    ensure!((1..=4096).contains(&config.updates), "updates must be 1..=4096");
    ensure!((1..=16).contains(&config.proof_workers), "proof_workers must be 1..=16");
    ensure!((1..=3600).contains(&config.timeout_secs), "timeout_secs must be 1..=3600");
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
    let factory = create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()));
    reth_db_common::init::init_genesis(&factory)?;
    let manager = factory.overlay_manager();
    let mut parent =
        factory.provider()?.sealed_header(0)?.ok_or_else(|| eyre::eyre!("missing genesis"))?;
    let strategy = DefaultStateRootStrategy::default();
    let tree_config = TreeConfig::default();
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
    let mut cold_starts = 1;
    for number in 1..=config.blocks {
        if number.is_multiple_of(7) {
            manager.clear_sparse_trie();
            cold_starts += 1;
        }
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
        let retained = blocks
            .iter()
            .filter(|block| block.recovered_block().number() > state_tip)
            .rev()
            .cloned()
            .collect();
        let (mut handle, pending) = strategy.spawn_state_root(
            &runtime,
            factory_view,
            StateRootTaskOptions {
                parent_header: parent.clone(),
                preserved_sparse_trie: manager.take_sparse_trie(),
                transaction_count: None,
                gas_used: None,
                config: &tree_config,
                pending_sparse_trie_prune_blocks: Some(retained),
            },
        );
        let stream = handle.take_hashed_update_stream();
        // Drop the best-effort sender too, so even empty streams close cleanly.
        drop(handle.take_hint_stream());
        let persist = number > 2 && (number.is_multiple_of(3) || number == config.blocks);
        let next_db = if persist { number - 1 } else { db_tip };
        let next_state = if !persist {
            state_tip
        } else if number.is_multiple_of(6) {
            next_db
        } else {
            next_db.saturating_sub(rng.random_range(1..=2)).max(state_tip)
        };
        let mut input = persist.then(|| {
            SaveBlocksInput::new(
                blocks[state_tip as usize..next_db as usize].to_vec(),
                db_tip,
                state_tip,
                next_db,
                next_state,
            )
        });
        // Dispatch after the first update, before finishing the stream. The scheduler
        // determines how commit interleaves with subsequent updates and proof reads.
        for update in updates {
            stream.on_hashed_state_update(update);
            if let Some(input) = input.take() {
                request_tx.send(input)?;
            }
        }
        // Empty blocks still need a persistence opportunity.
        if let Some(input) = input {
            request_tx.send(input)?;
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
        if persist {
            ack_rx.recv_timeout(timeout)??;
            db_tip = next_db;
            state_tip = next_state;
            commits += 1;
            partial_commits += u64::from(db_tip != state_tip);
            verify(&factory, &snapshots[db_tip as usize], db_tip, state_tip)?;
            manager.remove_blocks(
                blocks
                    .iter()
                    .filter(|block| block.recovered_block().number() <= state_tip)
                    .map(|block| block.recovered_block().hash()),
            );
        }
    }
    request_tx.send(SaveBlocksInput::new(
        blocks[state_tip as usize..].to_vec(),
        db_tip,
        state_tip,
        config.blocks,
        config.blocks,
    ))?;
    ack_rx.recv_timeout(timeout)??;
    commits += 1;
    verify(&factory, &state, config.blocks, config.blocks)?;
    drop(request_tx);
    writer.join().map_err(|_| eyre::eyre!("persistence worker panicked"))?;
    // Synchronize final trie completion before taking the metrics snapshot.
    if let Some(trie) = manager.take_sparse_trie() {
        ensure!(trie.into_trie_for(parent.hash())?.is_some(), "missing final reusable trie");
    }
    let metrics = metric_counts(snapshotter);
    ensure!(partial_commits > 0, "partial persistence never fired");
    for suffix in [
        "sparse_trie_reveal_multiproof_duration_histogram",
        "sparse_trie_prune_duration_histogram",
        "sparse_trie_cache_wait_duration_histogram",
        "overlay_computation_duration_seconds",
    ] {
        ensure!(
            metrics.iter().any(|(name, count)| name.ends_with(suffix) && *count > 0),
            "missing effect: {suffix}"
        );
    }
    println!(
        "{}",
        json!({"completed": true, "seed": config.seed, "blocks": config.blocks,
        "commits": commits, "partial_commits": partial_commits, "cold_starts": cold_starts,
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
