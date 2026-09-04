//! Schedule real engine-loop iterations, persistence completions, and payload leases.
//!
//! Persistence is modeled at its action/completion boundary. These scenarios exercise already
//! executed blocks, forkchoice, handoff, and shutdown; they do not simulate database writes or EVM
//! execution. The production event selector and persistence scheduling run on every iteration.

use super::*;
use alloy_consensus::Header;
use commonware_runtime::{deterministic, Runner, Supervisor};
use reth_tasks::TaskRuntime;

#[derive(Debug, PartialEq, Eq)]
struct HandoffOutcome {
    audit: String,
    trace: Vec<HandoffStep>,
    writes: Vec<(u64, u64, u64, u64)>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct HandoffStep {
    event: LoopStep,
    block_tip: u64,
    state_tip: u64,
    persistence_pending: bool,
    leases_active: bool,
    queued_messages: usize,
}

fn handoff_blocks() -> Vec<ExecutedBlock> {
    let mut parent_hash = B256::ZERO;
    (1..=6)
        .map(|number| {
            let block = Block {
                header: Header { number, parent_hash, ..Default::default() },
                body: Default::default(),
            }
            .seal_slow();
            parent_hash = block.hash();
            ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(block, Vec::new())),
                ..Default::default()
            }
        })
        .collect()
}

fn simulate_handoff(
    seed: u64,
    persistence_delay: u64,
    runtime: &reth_tasks::Runtime,
) -> HandoffOutcome {
    let blocks = handoff_blocks();
    let config = TreeConfig::default()
        .with_has_enough_parallelism(true)
        .with_cross_block_cache_size(1024 * 1024)
        .with_memory_block_buffer_target(0)
        .with_persistence_threshold(2)
        .with_persistence_backpressure_threshold(3)
        .with_num_state_masking_blocks(1);
    let (action_tx, action_rx) = std::sync::mpsc::channel();
    let mut harness =
        TestHarness::with_runtime(MAINNET.clone(), action_tx, action_rx, config, runtime.clone())
            .with_blocks(blocks[..3].to_vec());
    for block in &blocks[3..] {
        harness.tree.state.tree_state.insert_executed(block.clone());
    }
    let leases = [harness.tree.payload_builds.acquire(), harness.tree.payload_builds.acquire()];
    let action_rx = std::mem::replace(&mut harness.action_rx, std::sync::mpsc::channel().1);
    let to_tree = harness.to_tree_tx.clone();
    let heads = [blocks[2].recovered_block().hash(), blocks[5].recovered_block().hash()];
    let persistence_blocks = blocks.clone();
    let (terminate_tx, mut terminate_rx) = oneshot::channel();

    let config =
        deterministic::Config::default().with_seed(seed).with_timeout(Some(Duration::from_secs(5)));
    deterministic::Runner::new(config).start(|context| async move {
        let tasks = TaskRuntime::deterministic(context.child("engine"));
        let persistence_runtime = tasks.clone();
        let persistence = tasks.spawn("persistence", async move {
            let mut writes = Vec::new();
            for expected in [(0, 0, 3, 2), (3, 2, 6, 5), (6, 5, 6, 6)] {
                let action = loop {
                    match action_rx.try_recv() {
                        Ok(action) => break action,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            persistence_runtime.sleep(Duration::from_millis(1)).await;
                        }
                        Err(err) => panic!("persistence disconnected before {expected:?}: {err}"),
                    }
                };
                let PersistenceAction::SaveBlocks(input, sender) = action else {
                    panic!("expected automatic save, got {action:?}")
                };
                let frontiers = (
                    input.prev_db_tip(),
                    input.prev_partial_state_trie(),
                    input.new_db_tip(),
                    input.new_partial_state_trie(),
                );
                assert_eq!(frontiers, expected);
                assert_eq!(
                    input.last_block(),
                    persistence_blocks[expected.2 as usize - 1].recovered_block().num_hash()
                );
                let state_tip =
                    persistence_blocks[expected.3 as usize - 1].recovered_block().num_hash();
                persistence_runtime.sleep(Duration::from_millis(persistence_delay)).await;
                sender
                    .send(PersistenceResult {
                        last_block: input.last_block(),
                        last_state_trie_block: state_tip,
                        commit_duration: Some(Duration::ZERO),
                    })
                    .unwrap();
                writes.push(frontiers);
            }
            assert!(action_rx.try_recv().is_err());
            writes
        });
        let mut builders = Vec::new();
        for (index, lease) in leases.into_iter().enumerate() {
            let builder_runtime = tasks.clone();
            builders.push(tasks.spawn("payload_builder", async move {
                builder_runtime.sleep(Duration::from_millis(1 + index as u64 * 3)).await;
                drop(lease);
            }));
        }
        let input_runtime = tasks.clone();
        let input = tasks.spawn("engine_input", async move {
            for head_block_hash in heads {
                let (tx, rx) = oneshot::channel();
                to_tree
                    .send(FromEngine::Request(
                        BeaconEngineMessage::ForkchoiceUpdated {
                            state: ForkchoiceState {
                                head_block_hash,
                                safe_block_hash: B256::ZERO,
                                finalized_block_hash: B256::ZERO,
                            },
                            payload_attrs: None,
                            tx,
                        }
                        .into(),
                    ))
                    .unwrap();
                let response = rx.await.unwrap().unwrap().await.unwrap();
                assert!(response.payload_status.is_valid());
                input_runtime.sleep(Duration::from_millis(2)).await;
            }
        });

        let mut trace = Vec::new();
        let mut messages = 0;
        let mut terminate_tx = Some(terminate_tx);
        loop {
            let tree = &mut harness.tree;
            let event = tree.step(false);
            messages += usize::from(event == LoopStep::EngineMessage);
            let in_progress = tree.persistence_state.in_progress();
            let active = tree.payload_builds.is_active();
            let block_tip = tree.persistence_state.last_persisted_block.number;
            let state_tip = tree.persistence_state.last_state_trie_persisted_block.number;
            assert!(state_tip <= block_tip);
            assert!(block_tip <= tree.state.tree_state.canonical_block_number());
            let reclaimed_through = state_tip;
            for block in &blocks {
                let block = block.recovered_block();
                let retained = block.number() > reclaimed_through;
                assert_eq!(tree.state.tree_state.contains_hash(&block.hash()), retained);
                if block.number() <= tree.state.tree_state.canonical_block_number() {
                    assert_eq!(
                        tree.canonical_in_memory_state.state_by_hash(block.hash()).is_some(),
                        retained
                    );
                }
            }
            trace.push(HandoffStep {
                event,
                block_tip,
                state_tip,
                persistence_pending: in_progress,
                leases_active: active,
                queued_messages: tree.incoming.len(),
            });

            if event == LoopStep::Shutdown {
                assert!(terminate_tx.is_none(), "engine stopped before shutdown was requested");
                assert_eq!((block_tip, state_tip), (6, 6));
                assert_eq!(tree.incoming.len(), 1, "shutdown consumed subsequent engine input");
                assert!(terminate_rx.try_recv().is_ok());
                break
            }
            if block_tip == 6 &&
                !in_progress &&
                !active &&
                messages == 2 &&
                let Some(tx) = terminate_tx.take()
            {
                harness
                    .to_tree_tx
                    .send(FromEngine::Event(FromOrchestrator::Terminate { tx }))
                    .unwrap();
                harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
            }
            tasks.sleep(Duration::from_millis(1)).await;
        }
        let writes = persistence.await.unwrap();
        input.await.unwrap();
        for builder in builders {
            builder.await.unwrap();
        }
        HandoffOutcome { audit: context.auditor().state(), trace, writes }
    })
}

#[test]
fn deterministic_persistence_handoff() {
    let runtime = reth_tasks::Runtime::test();
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(err) => panic!("invalid RETH_DST_SEED: {err}"),
    };
    let campaign = seeds.len() > 1;
    let mut traces = std::collections::BTreeSet::new();
    for seed in seeds {
        for persistence_delay in [0, 20] {
            eprintln!("handoff seed={seed}, persistence_delay={persistence_delay}");
            let outcome = simulate_handoff(seed, persistence_delay, &runtime);
            assert_eq!(
                outcome,
                simulate_handoff(seed, persistence_delay, &runtime),
                "handoff replay diverged: seed={seed}, persistence_delay={persistence_delay}"
            );
            traces.insert(outcome.trace);
        }
    }
    assert!(traces.len() > 1, "persistence delays did not change the schedule");
    if campaign {
        assert!(traces
            .iter()
            .flatten()
            .any(|step| step.event == LoopStep::PersistenceComplete && !step.leases_active));
        assert!(traces.iter().flatten().any(|step| step.event == LoopStep::Idle &&
            step.persistence_pending &&
            step.queued_messages > 0));
    }
}

#[test]
fn engine_step_prioritizes_persistence_and_preserves_backpressure() {
    let blocks = handoff_blocks();
    let mut harness = TestHarness::with_config(
        MAINNET.clone(),
        TreeConfig::default()
            .with_persistence_threshold(2)
            .with_memory_block_buffer_target(0)
            .with_persistence_backpressure_threshold(3)
            .with_num_state_masking_blocks(1),
    )
    .with_blocks(blocks[..3].to_vec());
    assert_eq!(harness.tree.step(false), LoopStep::Idle);
    assert!(harness.action_rx.try_recv().is_err(), "idle polls must not start persistence");

    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::EngineMessage);
    let PersistenceAction::SaveBlocks(input, sender) = harness.action_rx.try_recv().unwrap() else {
        panic!("processing input should start persistence")
    };
    assert_eq!((input.new_db_tip(), input.new_partial_state_trie()), (3, 2));
    drop(harness.tree.payload_builds.acquire());
    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::Idle);
    assert!(harness.tree.persistence_state.in_progress());
    assert_eq!(harness.tree.incoming.len(), 1);
    assert_eq!(harness.tree.payload_build_finished.len(), 1);

    sender
        .send(PersistenceResult {
            last_block: input.last_block(),
            last_state_trie_block: blocks[1].recovered_block().num_hash(),
            commit_duration: None,
        })
        .unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::PersistenceComplete);
    assert_eq!(harness.tree.incoming.len(), 1);
    assert_eq!(harness.tree.step(false), LoopStep::PayloadBuildFinished);
    assert_eq!(harness.tree.incoming.len(), 1);
    assert_eq!(harness.tree.step(false), LoopStep::EngineMessage);
    assert!(harness.tree.incoming.is_empty());
    assert_eq!(harness.tree.step(false), LoopStep::Idle);
}

#[test]
fn engine_step_stops_on_disconnected_channels() {
    let mut harness = TestHarness::new(MAINNET.clone());
    let (sender, receiver) = crossbeam_channel::bounded(1);
    harness.tree.persistence_state.start_save(BlockNumHash::default(), receiver);
    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    drop(sender);
    assert_eq!(harness.tree.step(false), LoopStep::Shutdown);
    assert_eq!(harness.tree.incoming.len(), 1);

    let mut harness = TestHarness::new(MAINNET.clone());
    let (sender, receiver) = crossbeam_channel::unbounded();
    harness.tree.incoming = receiver;
    drop(sender);
    assert_eq!(harness.tree.step(false), LoopStep::Shutdown);
}

#[test]
fn engine_step_shutdown_drains_inflight_and_masked_state() {
    let blocks = handoff_blocks();
    let mut harness = TestHarness::with_config(
        MAINNET.clone(),
        TreeConfig::default()
            .with_persistence_threshold(2)
            .with_memory_block_buffer_target(0)
            .with_persistence_backpressure_threshold(10)
            .with_num_state_masking_blocks(1),
    )
    .with_blocks(blocks[..3].to_vec());
    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::EngineMessage);
    let PersistenceAction::SaveBlocks(input, sender) = harness.action_rx.try_recv().unwrap() else {
        panic!("expected threshold save")
    };
    let lease = harness.tree.payload_builds.acquire();
    let (tx, mut rx) = oneshot::channel();
    harness.to_tree_tx.send(FromEngine::Event(FromOrchestrator::Terminate { tx })).unwrap();
    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::Terminating);
    assert!(rx.try_recv().is_err());
    assert!(harness.action_rx.try_recv().is_err(), "must await the existing write");

    sender
        .send(PersistenceResult {
            last_block: input.last_block(),
            last_state_trie_block: blocks[1].recovered_block().num_hash(),
            commit_duration: None,
        })
        .unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::Terminating);
    let PersistenceAction::SaveBlocks(input, sender) = harness.action_rx.try_recv().unwrap() else {
        panic!("expected shutdown state/trie catch-up")
    };
    assert_eq!(
        (
            input.prev_db_tip(),
            input.prev_partial_state_trie(),
            input.new_db_tip(),
            input.new_partial_state_trie()
        ),
        (3, 2, 3, 3)
    );
    assert!(input.persist_rest_blocks().is_empty());
    assert_eq!(harness.tree.step(false), LoopStep::Terminating);
    assert!(rx.try_recv().is_err());
    sender
        .send(PersistenceResult {
            last_block: input.last_block(),
            last_state_trie_block: input.last_block(),
            commit_duration: None,
        })
        .unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::Shutdown);
    assert!(rx.try_recv().is_ok());
    assert_eq!(harness.tree.incoming.len(), 1);
    for block in &blocks[..3] {
        assert!(!harness.tree.state.tree_state.contains_hash(&block.recovered_block().hash()));
    }
    drop(lease);
}

#[test]
fn engine_step_shutdown_signals_after_persistence_failure() {
    let blocks = handoff_blocks();
    let mut harness = TestHarness::new(MAINNET.clone()).with_blocks(blocks[..3].to_vec());
    let (tx, mut rx) = oneshot::channel();
    harness.to_tree_tx.send(FromEngine::Event(FromOrchestrator::Terminate { tx })).unwrap();
    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::Terminating);
    let PersistenceAction::SaveBlocks(_, sender) = harness.action_rx.try_recv().unwrap() else {
        panic!("expected shutdown save")
    };
    drop(sender);
    assert_eq!(harness.tree.step(false), LoopStep::Shutdown);
    assert!(rx.try_recv().is_ok(), "failed persistence must still signal termination");
    assert_eq!(harness.tree.incoming.len(), 1);
}

#[test]
fn engine_step_shutdown_signals_when_handler_drops() {
    let blocks = handoff_blocks();
    let mut harness = TestHarness::new(MAINNET.clone()).with_blocks(blocks[..3].to_vec());
    let (tx, mut rx) = oneshot::channel();
    harness.to_tree_tx.send(FromEngine::Event(FromOrchestrator::Terminate { tx })).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::Terminating);
    assert!(rx.try_recv().is_err());
    drop(harness.tree);
    assert!(rx.try_recv().is_ok());
}

#[test]
fn engine_step_defers_explicit_persistence_wait_without_blocking() {
    let blocks = handoff_blocks();
    let mut harness = TestHarness::with_config(
        MAINNET.clone(),
        TreeConfig::default()
            .with_persistence_threshold(2)
            .with_memory_block_buffer_target(0)
            .with_persistence_backpressure_threshold(10)
            .with_num_state_masking_blocks(1),
    )
    .with_blocks(blocks.clone());
    let (complete, receiver) = crossbeam_channel::bounded(1);
    harness.tree.persistence_state.start_save(blocks[2].recovered_block().num_hash(), receiver);
    let (tx, mut response) = oneshot::channel();
    harness
        .to_tree_tx
        .send(FromEngine::Request(
            BeaconEngineMessage::RethNewPayload {
                payload: ExecutionData::new(
                    ExecutionPayloadV1::from_block_slow(&Block {
                        header: Header {
                            number: 7,
                            parent_hash: B256::repeat_byte(0x42),
                            ..Default::default()
                        },
                        body: Default::default(),
                    })
                    .into(),
                    ExecutionPayloadSidecar::none(),
                ),
                wait_for_persistence: true,
                wait_for_caches: false,
                tx,
                enqueued_at: std::time::Instant::now(),
            }
            .into(),
        ))
        .unwrap();
    harness.to_tree_tx.send(FromEngine::DownloadedBlocks(Vec::new())).unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::EngineMessage);
    assert!(harness.tree.deferred_engine_message.is_some());
    assert_eq!(harness.tree.step(false), LoopStep::Idle);
    assert!(response.try_recv().is_err());
    assert_eq!(harness.tree.incoming.len(), 1);
    complete
        .send(PersistenceResult {
            last_block: blocks[2].recovered_block().num_hash(),
            last_state_trie_block: blocks[1].recovered_block().num_hash(),
            commit_duration: None,
        })
        .unwrap();
    assert_eq!(harness.tree.step(false), LoopStep::PersistenceComplete);
    assert!(harness.action_rx.try_recv().is_err(), "execute deferred request before next save");
    drop(harness.tree.payload_builds.acquire());
    assert_eq!(harness.tree.step(false), LoopStep::PayloadBuildFinished);
    assert_eq!(harness.tree.step(false), LoopStep::EngineMessage);
    assert!(response.try_recv().is_ok());
    assert!(harness.tree.deferred_engine_message.is_none());
    assert_eq!(harness.tree.incoming.len(), 1, "deferred request retains FIFO order");
    assert!(matches!(harness.action_rx.try_recv(), Ok(PersistenceAction::SaveBlocks(..))));
}

#[test]
fn engine_step_cooperative_shutdown_advances_virtual_time() {
    let blocks = handoff_blocks();
    let mut harness = TestHarness::new(MAINNET.clone()).with_blocks(blocks[..3].to_vec());
    let (complete, receiver) = crossbeam_channel::bounded(1);
    let tip = blocks[2].recovered_block().num_hash();
    harness.tree.persistence_state.start_save(tip, receiver);
    let (terminate, terminated) = oneshot::channel();
    harness
        .to_tree_tx
        .send(FromEngine::Event(FromOrchestrator::Terminate { tx: terminate }))
        .unwrap();
    let config =
        deterministic::Config::default().with_seed(7).with_timeout(Some(Duration::from_secs(1)));
    deterministic::Runner::new(config).start(|context| async move {
        let tasks = TaskRuntime::deterministic(context.child("termination"));
        let persistence_runtime = tasks.clone();
        let persistence = tasks.spawn("delayed_persistence", async move {
            persistence_runtime.sleep(Duration::from_millis(20)).await;
            complete
                .send(PersistenceResult {
                    last_block: tip,
                    last_state_trie_block: tip,
                    commit_duration: None,
                })
                .unwrap();
        });
        let engine_runtime = tasks.clone();
        let engine = tasks.spawn("engine", harness.tree.run_cooperative(engine_runtime));
        terminated.await.unwrap();
        engine.await.unwrap();
        persistence.await.unwrap();
    });
}
