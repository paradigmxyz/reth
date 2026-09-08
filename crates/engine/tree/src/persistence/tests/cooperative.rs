//! Exercise the cooperative worker against real providers. Native backend work is atomic from
//! the simulator's perspective; no reader snapshot is held while an unwind waits for readers.

use super::*;
use alloy_consensus::Header;
use commonware_runtime::{deterministic, Runner, Supervisor};
use futures::FutureExt;
use reth_chainspec::ChainSpecBuilder;
use reth_ethereum_primitives::Block;
use reth_primitives_traits::{Block as _, RecoveredBlock};
use reth_provider::{
    test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
    ChainStateBlockReader, StageCheckpointReader, StorageSettings,
};
use reth_stages_api::StageId;

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    replies: Vec<(BlockNumHash, BlockNumHash, bool)>,
    sync_heights: Vec<u64>,
    safe: u64,
}

fn fixture() -> (ProviderFactory<MockNodeTypesWithDB>, Vec<ExecutedBlock>) {
    let spec = Arc::new(ChainSpecBuilder::mainnet().genesis(Default::default()).build());
    let provider = create_test_provider_factory_with_chain_spec(spec);
    provider.set_storage_settings_cache(StorageSettings::v2());
    let mut parent_hash = init_genesis(&provider).unwrap();
    let blocks = (1..=3)
        .map(|number| {
            let block = Block {
                header: Header {
                    number,
                    parent_hash,
                    timestamp: number,
                    gas_limit: 30_000_000,
                    ..Default::default()
                },
                body: Default::default(),
            }
            .seal_slow();
            parent_hash = block.hash();
            ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(block, Vec::new())),
                ..Default::default()
            }
        })
        .collect();
    (provider, blocks)
}

fn spawn(
    provider: &ProviderFactory<MockNodeTypesWithDB>,
    runtime: &TaskRuntime,
    sync_metrics_tx: &MetricEventsSender,
) -> (PersistenceHandle, TaskHandle<Result<(), PersistenceError>>) {
    let (_finished_exex_height_tx, finished_exex_height_rx) =
        tokio::sync::watch::channel(FinishedExExHeight::NoExExs);
    let pruner =
        Pruner::new_with_factory(provider.clone(), vec![], 5, 0, None, finished_exex_height_rx);
    PersistenceHandle::<EthPrimitives>::spawn_service_with_runtime(
        provider.clone(),
        pruner,
        sync_metrics_tx.clone(),
        runtime.clone(),
    )
}

async fn reply(
    runtime: &TaskRuntime,
    receiver: crossbeam_channel::Receiver<PersistenceResult>,
) -> PersistenceResult {
    loop {
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(crossbeam_channel::TryRecvError::Empty) => runtime.yield_now().await,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                panic!("worker exited without acknowledging persistence")
            }
        }
    }
}

fn frontiers(provider: &ProviderFactory<MockNodeTypesWithDB>) -> (u64, u64) {
    let checkpoint =
        provider.provider().unwrap().get_stage_checkpoint(StageId::Finish).unwrap().unwrap();
    let state = checkpoint
        .finish_stage_checkpoint()
        .and_then(|finish| finish.partial_state_trie())
        .unwrap_or(checkpoint.block_number);
    (checkpoint.block_number, state)
}

fn record(result: PersistenceResult) -> (BlockNumHash, BlockNumHash, bool) {
    (result.last_block, result.last_state_trie_block, result.commit_duration.is_some())
}

async fn workload(runtime: TaskRuntime) -> Outcome {
    let (provider, blocks) = fixture();
    let (sync_metrics_tx, mut sync_metrics_rx) = unbounded_channel();
    let (handle, mut worker) = spawn(&provider, &runtime, &sync_metrics_tx);
    let survivor = handle.clone();
    handle.save_finalized_block_number(2).unwrap();
    handle.save_safe_block_number(3).unwrap();
    let (sender, receiver) = crossbeam_channel::bounded(1);
    let initial_state_tip = 1;
    handle
        .save_blocks(SaveBlocksInput::new(blocks[..2].to_vec(), 0, 0, 2, initial_state_tip), sender)
        .unwrap();
    let saved = reply(&runtime, receiver).await;
    assert_eq!(saved.last_block, blocks[1].recovered_block().num_hash());
    assert_eq!(
        saved.last_state_trie_block,
        blocks[initial_state_tip as usize - 1].recovered_block().num_hash()
    );
    assert_eq!(frontiers(&provider), (2, initial_state_tip));
    {
        let reader = provider.provider().unwrap();
        assert_eq!(reader.last_finalized_block_number().unwrap(), Some(2));
        assert_eq!(reader.last_safe_block_number().unwrap(), Some(2));
    }

    let mut replies = vec![record(saved)];
    {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        handle
            .save_blocks(SaveBlocksInput::new(vec![blocks[1].clone()], 2, 1, 2, 2), sender)
            .unwrap();
        let caught_up = reply(&runtime, receiver).await;
        assert_eq!(caught_up.last_block, blocks[1].recovered_block().num_hash());
        assert_eq!(caught_up.last_state_trie_block, caught_up.last_block);
        replies.push(record(caught_up));
    }
    assert_eq!(frontiers(&provider), (2, 2));
    drop(handle);
    assert!((&mut worker).now_or_never().is_none(), "a live handle must keep the worker open");

    // Queue dependent operations, then close input. Shutdown must drain both, and acknowledgments
    // must reflect the actual remove/save order even if their receivers are not polled yet.
    let (removed_tx, removed_rx) = crossbeam_channel::bounded(1);
    survivor.remove_blocks_above(1, removed_tx).unwrap();
    let (saved_tx, saved_rx) = crossbeam_channel::bounded(1);
    survivor.save_blocks(SaveBlocksInput::new(blocks[1..].to_vec(), 1, 1, 3, 3), saved_tx).unwrap();
    drop(survivor);
    worker.await.unwrap().unwrap();
    let removed = removed_rx.try_recv().unwrap();
    assert_eq!(removed.last_block, blocks[0].recovered_block().num_hash());
    assert_eq!(removed.last_state_trie_block, removed.last_block);
    assert!(removed.commit_duration.is_none());
    let saved_again = saved_rx.try_recv().unwrap();
    assert_eq!(saved_again.last_block, blocks[2].recovered_block().num_hash());
    assert_eq!(saved_again.last_state_trie_block, saved_again.last_block);
    assert_eq!(frontiers(&provider), (3, 3));
    let safe = provider.provider().unwrap().last_safe_block_number().unwrap().unwrap();
    assert_eq!(safe, 3, "a future safe height must remain pending until its block is persisted");

    // Restart the service using the persisted frontier; the old worker has released its providers.
    let (restarted, worker) = spawn(&provider, &runtime, &sync_metrics_tx);
    let (sender, receiver) = crossbeam_channel::bounded(1);
    restarted.remove_blocks_above(2, sender).unwrap();
    drop(restarted);
    worker.await.unwrap().unwrap();
    let restarted_result = receiver.try_recv().unwrap();
    assert_eq!(restarted_result.last_block, blocks[1].recovered_block().num_hash());
    assert_eq!(frontiers(&provider), (2, 2));
    assert!(provider.provider().unwrap().header_by_number(3).unwrap().is_none());
    assert_eq!(provider.check_consistency().unwrap(), (None, None));

    // An actual provider error terminates the worker and closes the reply without a success value.
    let (failed, worker) = spawn(&provider, &runtime, &sync_metrics_tx);
    let (sender, receiver) = crossbeam_channel::bounded(1);
    failed.remove_blocks_above(99, sender).unwrap();
    assert!(matches!(
        worker.await.unwrap(),
        Err(PersistenceError::ProviderError(ProviderError::HeaderNotFound(_)))
    ));
    assert!(matches!(receiver.try_recv(), Err(crossbeam_channel::TryRecvError::Disconnected)));
    assert!(failed.save_safe_block_number(2).is_err());
    assert_eq!(frontiers(&provider), (2, 2));

    let mut sync_heights = Vec::new();
    while let Ok(MetricEvent::SyncHeight { height }) = sync_metrics_rx.try_recv() {
        sync_heights.push(height);
    }
    assert_eq!(sync_heights, vec![2, 1, 3, 2]);
    replies.extend([removed, saved_again, restarted_result].into_iter().map(record));
    Outcome { replies, sync_heights, safe }
}

#[test]
fn production_persistence_worker_actions() {
    let runtime = reth_tasks::Runtime::test();
    let _ = futures::executor::block_on(workload(TaskRuntime::from(runtime)));
}

#[test]
fn deterministic_persistence_worker_actions() {
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    for seed in seeds {
        eprintln!("persistence worker seed={seed}");
        let simulate = || {
            deterministic::Runner::new(
                deterministic::Config::default()
                    .with_seed(seed)
                    .with_timeout(Some(Duration::from_secs(10))),
            )
            .start(|context| async move {
                let result =
                    workload(TaskRuntime::deterministic(context.child("persistence_worker"))).await;
                (context.auditor().state(), result)
            })
        };
        assert_eq!(simulate(), simulate(), "persistence worker replay seed={seed}");
    }
}
