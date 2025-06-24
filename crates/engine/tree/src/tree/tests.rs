use super::*;
use crate::persistence::PersistenceAction;
use alloy_consensus::Header;
use alloy_primitives::{
    map::{HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::Decodable;
use alloy_rpc_types_engine::{ExecutionData, ExecutionPayloadSidecar, ExecutionPayloadV1};
use assert_matches::assert_matches;
use reth_chain_state::{test_utils::TestBlockBuilder, BlockState};
use reth_chainspec::{ChainSpec, HOLESKY, MAINNET};
use reth_engine_primitives::ForkchoiceStatus;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm_ethereum::MockEvmConfig;
use reth_node_ethereum::EthereumEngineValidator;
use reth_primitives_traits::Block as _;
use reth_provider::test_utils::MockEthProvider;
use reth_trie::HashedPostState;
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::mpsc::{channel, Sender},
};

/// This is a test channel that allows you to `release` any value that is in the channel.
///
/// If nothing has been sent, then the next value will be immediately sent.
struct TestChannel<T> {
    /// If an item is sent to this channel, an item will be released in the wrapped channel
    release: Receiver<()>,
    /// The sender channel
    tx: Sender<T>,
    /// The receiver channel
    rx: Receiver<T>,
}

impl<T: Send + 'static> TestChannel<T> {
    /// Creates a new test channel
    fn spawn_channel() -> (Sender<T>, Receiver<T>, TestChannelHandle) {
        let (original_tx, original_rx) = channel();
        let (wrapped_tx, wrapped_rx) = channel();
        let (release_tx, release_rx) = channel();
        let handle = TestChannelHandle::new(release_tx);
        let test_channel = Self { release: release_rx, tx: wrapped_tx, rx: original_rx };
        // spawn the task that listens and releases stuff
        std::thread::spawn(move || test_channel.intercept_loop());
        (original_tx, wrapped_rx, handle)
    }

    /// Runs the intercept loop, waiting for the handle to release a value
    fn intercept_loop(&self) {
        while self.release.recv() == Ok(()) {
            let Ok(value) = self.rx.recv() else { return };

            let _ = self.tx.send(value);
        }
    }
}

struct TestChannelHandle {
    /// The sender to use for releasing values
    release: Sender<()>,
}

impl TestChannelHandle {
    /// Returns a [`TestChannelHandle`]
    const fn new(release: Sender<()>) -> Self {
        Self { release }
    }

    /// Signals to the channel task that a value should be released
    #[expect(dead_code)]
    fn release(&self) {
        let _ = self.release.send(());
    }
}

struct TestHarness {
    tree: EngineApiTreeHandler<
        EthPrimitives,
        MockEthProvider,
        EthEngineTypes,
        EthereumEngineValidator,
        MockEvmConfig,
    >,
    to_tree_tx: Sender<FromEngine<EngineApiRequest<EthEngineTypes, EthPrimitives>, Block>>,
    from_tree_rx: UnboundedReceiver<EngineApiEvent>,
    blocks: Vec<ExecutedBlockWithTrieUpdates>,
    action_rx: Receiver<PersistenceAction>,
    block_builder: TestBlockBuilder,
    provider: MockEthProvider,
}

impl TestHarness {
    fn new(chain_spec: Arc<ChainSpec>) -> Self {
        let (action_tx, action_rx) = channel();
        Self::with_persistence_channel(chain_spec, action_tx, action_rx)
    }

    #[expect(dead_code)]
    fn with_test_channel(chain_spec: Arc<ChainSpec>) -> (Self, TestChannelHandle) {
        let (action_tx, action_rx, handle) = TestChannel::spawn_channel();
        (Self::with_persistence_channel(chain_spec, action_tx, action_rx), handle)
    }

    fn with_persistence_channel(
        chain_spec: Arc<ChainSpec>,
        action_tx: Sender<PersistenceAction>,
        action_rx: Receiver<PersistenceAction>,
    ) -> Self {
        let persistence_handle = PersistenceHandle::new(action_tx);

        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

        let provider = MockEthProvider::default();

        let payload_validator = EthereumEngineValidator::new(chain_spec.clone());

        let (from_tree_tx, from_tree_rx) = unbounded_channel();

        let header = chain_spec.genesis_header().clone();
        let header = SealedHeader::seal_slow(header);
        let engine_api_tree_state =
            EngineApiTreeState::new(10, 10, header.num_hash(), EngineApiKind::Ethereum);
        let canonical_in_memory_state = CanonicalInMemoryState::with_head(header, None, None);

        let (to_payload_service, _payload_command_rx) = unbounded_channel();
        let payload_builder = PayloadBuilderHandle::new(to_payload_service);

        let evm_config = MockEvmConfig::default();

        let tree = EngineApiTreeHandler::new(
            provider.clone(),
            consensus,
            payload_validator,
            from_tree_tx,
            engine_api_tree_state,
            canonical_in_memory_state,
            persistence_handle,
            PersistenceState::default(),
            payload_builder,
            // always assume enough parallelism for tests
            TreeConfig::default().with_legacy_state_root(false).with_has_enough_parallelism(true),
            EngineApiKind::Ethereum,
            evm_config,
        );

        let block_builder = TestBlockBuilder::default().with_chain_spec((*chain_spec).clone());
        Self {
            to_tree_tx: tree.incoming_tx.clone(),
            tree,
            from_tree_rx,
            blocks: vec![],
            action_rx,
            block_builder,
            provider,
        }
    }

    fn with_blocks(mut self, blocks: Vec<ExecutedBlockWithTrieUpdates>) -> Self {
        let mut blocks_by_hash = HashMap::default();
        let mut blocks_by_number = BTreeMap::new();
        let mut state_by_hash = HashMap::default();
        let mut hash_by_number = BTreeMap::new();
        let mut parent_to_child: HashMap<B256, HashSet<B256>> = HashMap::default();
        let mut parent_hash = B256::ZERO;

        for block in &blocks {
            let sealed_block = block.recovered_block();
            let hash = sealed_block.hash();
            let number = sealed_block.number;
            blocks_by_hash.insert(hash, block.clone());
            blocks_by_number.entry(number).or_insert_with(Vec::new).push(block.clone());
            state_by_hash.insert(hash, Arc::new(BlockState::new(block.clone())));
            hash_by_number.insert(number, hash);
            parent_to_child.entry(parent_hash).or_default().insert(hash);
            parent_hash = hash;
        }

        self.tree.state.tree_state = TreeState {
            blocks_by_hash,
            blocks_by_number,
            current_canonical_head: blocks.last().unwrap().recovered_block().num_hash(),
            parent_to_child,
            persisted_trie_updates: HashMap::default(),
            engine_kind: EngineApiKind::Ethereum,
        };

        let last_executed_block = blocks.last().unwrap().clone();
        let pending = Some(BlockState::new(last_executed_block));
        self.tree.canonical_in_memory_state =
            CanonicalInMemoryState::new(state_by_hash, hash_by_number, pending, None, None);

        self.blocks = blocks.clone();

        let recovered_blocks =
            blocks.iter().map(|b| b.recovered_block().clone()).collect::<Vec<_>>();

        self.persist_blocks(recovered_blocks);

        self
    }

    const fn with_backfill_state(mut self, state: BackfillSyncState) -> Self {
        self.tree.backfill_sync_state = state;
        self
    }

    async fn fcu_to(&mut self, block_hash: B256, fcu_status: impl Into<ForkchoiceStatus>) {
        let fcu_status = fcu_status.into();

        self.send_fcu(block_hash, fcu_status).await;

        self.check_fcu(block_hash, fcu_status).await;
    }

    async fn send_fcu(&mut self, block_hash: B256, fcu_status: impl Into<ForkchoiceStatus>) {
        let fcu_state = self.fcu_state(block_hash);

        let (tx, rx) = oneshot::channel();
        self.tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: fcu_state,
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        match fcu_status.into() {
            ForkchoiceStatus::Valid => assert!(response.payload_status.is_valid()),
            ForkchoiceStatus::Syncing => assert!(response.payload_status.is_syncing()),
            ForkchoiceStatus::Invalid => assert!(response.payload_status.is_invalid()),
        }
    }

    async fn check_fcu(&mut self, block_hash: B256, fcu_status: impl Into<ForkchoiceStatus>) {
        let fcu_state = self.fcu_state(block_hash);

        // check for ForkchoiceUpdated event
        let event = self.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::BeaconConsensus(BeaconConsensusEngineEvent::ForkchoiceUpdated(
                state,
                status,
            )) => {
                assert_eq!(state, fcu_state);
                assert_eq!(status, fcu_status.into());
            }
            _ => panic!("Unexpected event: {event:#?}"),
        }
    }

    const fn fcu_state(&self, block_hash: B256) -> ForkchoiceState {
        ForkchoiceState {
            head_block_hash: block_hash,
            safe_block_hash: block_hash,
            finalized_block_hash: block_hash,
        }
    }

    fn persist_blocks(&self, blocks: Vec<RecoveredBlock<reth_ethereum_primitives::Block>>) {
        let mut block_data: Vec<(B256, Block)> = Vec::with_capacity(blocks.len());
        let mut headers_data: Vec<(B256, Header)> = Vec::with_capacity(blocks.len());

        for block in &blocks {
            block_data.push((block.hash(), block.clone_block()));
            headers_data.push((block.hash(), block.header().clone()));
        }

        self.provider.extend_blocks(block_data);
        self.provider.extend_headers(headers_data);
    }
}

#[test]
fn test_tree_persist_block_batch() {
    let tree_config = TreeConfig::default();
    let chain_spec = MAINNET.clone();
    let mut test_block_builder = TestBlockBuilder::eth().with_chain_spec((*chain_spec).clone());

    // we need more than tree_config.persistence_threshold() +1 blocks to
    // trigger the persistence task.
    let blocks: Vec<_> = test_block_builder
        .get_executed_blocks(1..tree_config.persistence_threshold() + 2)
        .collect();
    let mut test_harness = TestHarness::new(chain_spec).with_blocks(blocks);

    let mut blocks = vec![];
    for idx in 0..tree_config.max_execute_block_batch_size() * 2 {
        blocks.push(test_block_builder.generate_random_block(idx as u64, B256::random()));
    }

    test_harness.to_tree_tx.send(FromEngine::DownloadedBlocks(blocks)).unwrap();

    // process the message
    let msg = test_harness.tree.try_recv_engine_message().unwrap().unwrap();
    test_harness.tree.on_engine_message(msg).unwrap();

    // we now should receive the other batch
    let msg = test_harness.tree.try_recv_engine_message().unwrap().unwrap();
    match msg {
        FromEngine::DownloadedBlocks(blocks) => {
            assert_eq!(blocks.len(), tree_config.max_execute_block_batch_size());
        }
        _ => panic!("unexpected message: {msg:#?}"),
    }
}

#[tokio::test]
async fn test_tree_persist_blocks() {
    let tree_config = TreeConfig::default();
    let chain_spec = MAINNET.clone();
    let mut test_block_builder = TestBlockBuilder::eth().with_chain_spec((*chain_spec).clone());

    // we need more than tree_config.persistence_threshold() +1 blocks to
    // trigger the persistence task.
    let blocks: Vec<_> = test_block_builder
        .get_executed_blocks(1..tree_config.persistence_threshold() + 2)
        .collect();
    let test_harness = TestHarness::new(chain_spec).with_blocks(blocks.clone());
    std::thread::Builder::new()
        .name("Tree Task".to_string())
        .spawn(|| test_harness.tree.run())
        .unwrap();

    // send a message to the tree to enter the main loop.
    test_harness.to_tree_tx.send(FromEngine::DownloadedBlocks(vec![])).unwrap();

    let received_action =
        test_harness.action_rx.recv().expect("Failed to receive save blocks action");
    if let PersistenceAction::SaveBlocks(saved_blocks, _) = received_action {
        // only blocks.len() - tree_config.memory_block_buffer_target() will be
        // persisted
        let expected_persist_len = blocks.len() - tree_config.memory_block_buffer_target() as usize;
        assert_eq!(saved_blocks.len(), expected_persist_len);
        assert_eq!(saved_blocks, blocks[..expected_persist_len]);
    } else {
        panic!("unexpected action received {received_action:?}");
    }
}

#[tokio::test]
async fn test_in_memory_state_trait_impl() {
    let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..10).collect();
    let test_harness = TestHarness::new(MAINNET.clone()).with_blocks(blocks.clone());

    for executed_block in blocks {
        let sealed_block = executed_block.recovered_block();

        let expected_state = BlockState::new(executed_block.clone());

        let actual_state_by_hash =
            test_harness.tree.canonical_in_memory_state.state_by_hash(sealed_block.hash()).unwrap();
        assert_eq!(expected_state, *actual_state_by_hash);

        let actual_state_by_number = test_harness
            .tree
            .canonical_in_memory_state
            .state_by_number(sealed_block.number)
            .unwrap();
        assert_eq!(expected_state, *actual_state_by_number);
    }
}

#[tokio::test]
async fn test_engine_request_during_backfill() {
    let tree_config = TreeConfig::default();
    let blocks: Vec<_> = TestBlockBuilder::eth()
        .get_executed_blocks(0..tree_config.persistence_threshold())
        .collect();
    let mut test_harness = TestHarness::new(MAINNET.clone())
        .with_blocks(blocks)
        .with_backfill_state(BackfillSyncState::Active);

    let (tx, rx) = oneshot::channel();
    test_harness
        .tree
        .on_engine_message(FromEngine::Request(
            BeaconEngineMessage::ForkchoiceUpdated {
                state: ForkchoiceState {
                    head_block_hash: B256::random(),
                    safe_block_hash: B256::random(),
                    finalized_block_hash: B256::random(),
                },
                payload_attrs: None,
                tx,
                version: EngineApiMessageVersion::default(),
            }
            .into(),
        ))
        .unwrap();

    let resp = rx.await.unwrap().unwrap().await.unwrap();
    assert!(resp.payload_status.is_syncing());
}

#[test]
fn test_disconnected_payload() {
    let s = include_str!("../../test-data/holesky/2.rlp");
    let data = Bytes::from_str(s).unwrap();
    let block = Block::decode(&mut data.as_ref()).unwrap();
    let sealed = block.seal_slow();
    let hash = sealed.hash();
    let payload = ExecutionPayloadV1::from_block_unchecked(hash, &sealed.clone().into_block());

    let mut test_harness = TestHarness::new(HOLESKY.clone());

    let outcome = test_harness
        .tree
        .on_new_payload(ExecutionData {
            payload: payload.into(),
            sidecar: ExecutionPayloadSidecar::none(),
        })
        .unwrap();
    assert!(outcome.outcome.is_syncing());

    // ensure block is buffered
    let buffered = test_harness.tree.state.buffer.block(&hash).unwrap();
    assert_eq!(buffered.clone_sealed_block(), sealed);
}

#[test]
fn test_disconnected_block() {
    let s = include_str!("../../test-data/holesky/2.rlp");
    let data = Bytes::from_str(s).unwrap();
    let block = Block::decode(&mut data.as_ref()).unwrap();
    let sealed = block.seal_slow().try_recover().unwrap();

    let mut test_harness = TestHarness::new(HOLESKY.clone());

    let outcome = test_harness.tree.insert_block(sealed.clone()).unwrap();
    assert_eq!(
        outcome,
        InsertPayloadOk::Inserted(BlockStatus::Disconnected {
            head: test_harness.tree.state.tree_state.current_canonical_head,
            missing_ancestor: sealed.parent_num_hash()
        })
    );
}

#[tokio::test]
async fn test_holesky_payload() {
    let s = include_str!("../../test-data/holesky/1.rlp");
    let data = Bytes::from_str(s).unwrap();
    let block: Block = Block::decode(&mut data.as_ref()).unwrap();
    let sealed = block.seal_slow();
    let payload =
        ExecutionPayloadV1::from_block_unchecked(sealed.hash(), &sealed.clone().into_block());

    let mut test_harness =
        TestHarness::new(HOLESKY.clone()).with_backfill_state(BackfillSyncState::Active);

    let (tx, rx) = oneshot::channel();
    test_harness
        .tree
        .on_engine_message(FromEngine::Request(
            BeaconEngineMessage::NewPayload {
                payload: ExecutionData {
                    payload: payload.clone().into(),
                    sidecar: ExecutionPayloadSidecar::none(),
                },
                tx,
            }
            .into(),
        ))
        .unwrap();

    let resp = rx.await.unwrap().unwrap();
    assert!(resp.is_syncing());
}

#[tokio::test]
async fn test_tree_state_on_new_head_reorg() {
    reth_tracing::init_test_tracing();
    let chain_spec = MAINNET.clone();

    // Set persistence_threshold to 1
    let mut test_harness = TestHarness::new(chain_spec);
    test_harness.tree.config =
        test_harness.tree.config.with_persistence_threshold(1).with_memory_block_buffer_target(1);
    let mut test_block_builder = TestBlockBuilder::eth();
    let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..6).collect();

    for block in &blocks {
        test_harness.tree.state.tree_state.insert_executed(block.clone());
    }

    // set block 3 as the current canonical head
    test_harness.tree.state.tree_state.set_canonical_head(blocks[2].recovered_block().num_hash());

    // create a fork from block 2
    let fork_block_3 =
        test_block_builder.get_executed_block_with_number(3, blocks[1].recovered_block().hash());
    let fork_block_4 =
        test_block_builder.get_executed_block_with_number(4, fork_block_3.recovered_block().hash());
    let fork_block_5 =
        test_block_builder.get_executed_block_with_number(5, fork_block_4.recovered_block().hash());

    test_harness.tree.state.tree_state.insert_executed(fork_block_3.clone());
    test_harness.tree.state.tree_state.insert_executed(fork_block_4.clone());
    test_harness.tree.state.tree_state.insert_executed(fork_block_5.clone());

    // normal (non-reorg) case
    let result = test_harness.tree.on_new_head(blocks[4].recovered_block().hash()).unwrap();
    assert!(matches!(result, Some(NewCanonicalChain::Commit { .. })));
    if let Some(NewCanonicalChain::Commit { new }) = result {
        assert_eq!(new.len(), 2);
        assert_eq!(new[0].recovered_block().hash(), blocks[3].recovered_block().hash());
        assert_eq!(new[1].recovered_block().hash(), blocks[4].recovered_block().hash());
    }

    // should be a None persistence action before we advance persistence
    let current_action = test_harness.tree.persistence_state.current_action();
    assert_eq!(current_action, None);

    // let's attempt to persist and check that it attempts to save blocks
    //
    // since in-memory block buffer target and persistence_threshold are both 1, this should
    // save all but the current tip of the canonical chain (up to blocks[1])
    test_harness.tree.advance_persistence().unwrap();
    let current_action = test_harness.tree.persistence_state.current_action().cloned();
    assert_eq!(
        current_action,
        Some(CurrentPersistenceAction::SavingBlocks {
            highest: blocks[1].recovered_block().num_hash()
        })
    );

    // get rid of the prev action
    let received_action = test_harness.action_rx.recv().unwrap();
    let PersistenceAction::SaveBlocks(saved_blocks, sender) = received_action else {
        panic!("received wrong action");
    };
    assert_eq!(saved_blocks, vec![blocks[0].clone(), blocks[1].clone()]);

    // send the response so we can advance again
    sender.send(Some(blocks[1].recovered_block().num_hash())).unwrap();

    // we should be persisting blocks[1] because we threw out the prev action
    let current_action = test_harness.tree.persistence_state.current_action().cloned();
    assert_eq!(
        current_action,
        Some(CurrentPersistenceAction::SavingBlocks {
            highest: blocks[1].recovered_block().num_hash()
        })
    );

    // after advancing persistence, we should be at `None` for the next action
    test_harness.tree.advance_persistence().unwrap();
    let current_action = test_harness.tree.persistence_state.current_action().cloned();
    assert_eq!(current_action, None);

    // reorg case
    let result = test_harness.tree.on_new_head(fork_block_5.recovered_block().hash()).unwrap();
    assert!(matches!(result, Some(NewCanonicalChain::Reorg { .. })));

    if let Some(NewCanonicalChain::Reorg { new, old }) = result {
        assert_eq!(new.len(), 3);
        assert_eq!(new[0].recovered_block().hash(), fork_block_3.recovered_block().hash());
        assert_eq!(new[1].recovered_block().hash(), fork_block_4.recovered_block().hash());
        assert_eq!(new[2].recovered_block().hash(), fork_block_5.recovered_block().hash());

        assert_eq!(old.len(), 1);
        assert_eq!(old[0].recovered_block().hash(), blocks[2].recovered_block().hash());
    }

    // The canonical block has not changed, so we will not get any active persistence action
    test_harness.tree.advance_persistence().unwrap();
    let current_action = test_harness.tree.persistence_state.current_action().cloned();
    assert_eq!(current_action, None);

    // Let's change the canonical head and advance persistence
    test_harness
        .tree
        .state
        .tree_state
        .set_canonical_head(fork_block_5.recovered_block().num_hash());

    // The canonical block has changed now, we should get fork_block_4 due to the persistence
    // threshold and in memory block buffer target
    test_harness.tree.advance_persistence().unwrap();
    let current_action = test_harness.tree.persistence_state.current_action().cloned();
    assert_eq!(
        current_action,
        Some(CurrentPersistenceAction::SavingBlocks {
            highest: fork_block_4.recovered_block().num_hash()
        })
    );
}

#[test]
fn test_tree_state_on_new_head_deep_fork() {
    reth_tracing::init_test_tracing();

    let chain_spec = MAINNET.clone();
    let mut test_harness = TestHarness::new(chain_spec);
    let mut test_block_builder = TestBlockBuilder::eth();

    let blocks: Vec<_> = test_block_builder.get_executed_blocks(0..5).collect();

    for block in &blocks {
        test_harness.tree.state.tree_state.insert_executed(block.clone());
    }

    // set last block as the current canonical head
    let last_block = blocks.last().unwrap().recovered_block().clone();

    test_harness.tree.state.tree_state.set_canonical_head(last_block.num_hash());

    // create a fork chain from last_block
    let chain_a = test_block_builder.create_fork(&last_block, 10);
    let chain_b = test_block_builder.create_fork(&last_block, 10);

    for block in &chain_a {
        test_harness.tree.state.tree_state.insert_executed(ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(block.clone()),
                execution_output: Arc::new(ExecutionOutcome::default()),
                hashed_state: Arc::new(HashedPostState::default()),
            },
            trie: ExecutedTrieUpdates::empty(),
        });
    }
    test_harness.tree.state.tree_state.set_canonical_head(chain_a.last().unwrap().num_hash());

    for block in &chain_b {
        test_harness.tree.state.tree_state.insert_executed(ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(block.clone()),
                execution_output: Arc::new(ExecutionOutcome::default()),
                hashed_state: Arc::new(HashedPostState::default()),
            },
            trie: ExecutedTrieUpdates::empty(),
        });
    }

    // for each block in chain_b, reorg to it and then back to canonical
    let mut expected_new = Vec::new();
    for block in &chain_b {
        // reorg to chain from block b
        let result = test_harness.tree.on_new_head(block.hash()).unwrap();
        assert_matches!(result, Some(NewCanonicalChain::Reorg { .. }));

        expected_new.push(block);
        if let Some(NewCanonicalChain::Reorg { new, old }) = result {
            assert_eq!(new.len(), expected_new.len());
            for (index, block) in expected_new.iter().enumerate() {
                assert_eq!(new[index].recovered_block().hash(), block.hash());
            }

            assert_eq!(old.len(), chain_a.len());
            for (index, block) in chain_a.iter().enumerate() {
                assert_eq!(old[index].recovered_block().hash(), block.hash());
            }
        }

        // set last block of chain a as canonical head
        test_harness.tree.on_new_head(chain_a.last().unwrap().hash()).unwrap();
    }
}

#[tokio::test]
async fn test_get_canonical_blocks_to_persist() {
    let chain_spec = MAINNET.clone();
    let mut test_harness = TestHarness::new(chain_spec);
    let mut test_block_builder = TestBlockBuilder::eth();

    let canonical_head_number = 9;
    let blocks: Vec<_> =
        test_block_builder.get_executed_blocks(0..canonical_head_number + 1).collect();
    test_harness = test_harness.with_blocks(blocks.clone());

    let last_persisted_block_number = 3;
    test_harness.tree.persistence_state.last_persisted_block =
        blocks[last_persisted_block_number as usize].recovered_block.num_hash();

    let persistence_threshold = 4;
    let memory_block_buffer_target = 3;
    test_harness.tree.config = TreeConfig::default()
        .with_persistence_threshold(persistence_threshold)
        .with_memory_block_buffer_target(memory_block_buffer_target);

    let blocks_to_persist = test_harness.tree.get_canonical_blocks_to_persist().unwrap();

    let expected_blocks_to_persist_length: usize =
        (canonical_head_number - memory_block_buffer_target - last_persisted_block_number)
            .try_into()
            .unwrap();

    assert_eq!(blocks_to_persist.len(), expected_blocks_to_persist_length);
    for (i, item) in blocks_to_persist.iter().enumerate().take(expected_blocks_to_persist_length) {
        assert_eq!(item.recovered_block().number, last_persisted_block_number + i as u64 + 1);
    }

    // make sure only canonical blocks are included
    let fork_block = test_block_builder.get_executed_block_with_number(4, B256::random());
    let fork_block_hash = fork_block.recovered_block().hash();
    test_harness.tree.state.tree_state.insert_executed(fork_block);

    assert!(test_harness.tree.state.tree_state.block_by_hash(fork_block_hash).is_some());

    let blocks_to_persist = test_harness.tree.get_canonical_blocks_to_persist().unwrap();
    assert_eq!(blocks_to_persist.len(), expected_blocks_to_persist_length);

    // check that the fork block is not included in the blocks to persist
    assert!(!blocks_to_persist.iter().any(|b| b.recovered_block().hash() == fork_block_hash));

    // check that the original block 4 is still included
    assert!(blocks_to_persist.iter().any(|b| b.recovered_block().number == 4 &&
        b.recovered_block().hash() == blocks[4].recovered_block().hash()));

    // check that if we advance persistence, the persistence action is the correct value
    test_harness.tree.advance_persistence().expect("advancing persistence should succeed");
    assert_eq!(
        test_harness.tree.persistence_state.current_action().cloned(),
        Some(CurrentPersistenceAction::SavingBlocks {
            highest: blocks_to_persist.last().unwrap().recovered_block().num_hash()
        })
    );
}

#[tokio::test]
async fn test_engine_tree_fcu_missing_head() {
    let chain_spec = MAINNET.clone();
    let mut test_harness = TestHarness::new(chain_spec.clone());

    let mut test_block_builder = TestBlockBuilder::eth().with_chain_spec((*chain_spec).clone());

    let blocks: Vec<_> = test_block_builder.get_executed_blocks(0..5).collect();
    test_harness = test_harness.with_blocks(blocks);

    let missing_block = test_block_builder
        .generate_random_block(6, test_harness.blocks.last().unwrap().recovered_block().hash());

    test_harness.fcu_to(missing_block.hash(), PayloadStatusEnum::Syncing).await;

    // after FCU we receive an EngineApiEvent::Download event to get the missing block.
    let event = test_harness.from_tree_rx.recv().await.unwrap();
    match event {
        EngineApiEvent::Download(DownloadRequest::BlockSet(actual_block_set)) => {
            let expected_block_set = HashSet::from_iter([missing_block.hash()]);
            assert_eq!(actual_block_set, expected_block_set);
        }
        _ => panic!("Unexpected event: {event:#?}"),
    }
}

#[tokio::test]
async fn test_engine_tree_live_sync_transition_required_blocks_requested() {
    reth_tracing::init_test_tracing();

    let chain_spec = MAINNET.clone();
    let mut test_harness = TestHarness::new(chain_spec.clone());

    let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
    test_harness = test_harness.with_blocks(base_chain.clone());

    test_harness
        .fcu_to(base_chain.last().unwrap().recovered_block().hash(), ForkchoiceStatus::Valid)
        .await;

    // extend main chain with enough blocks to trigger pipeline run but don't insert them
    let main_chain = test_harness
        .block_builder
        .create_fork(base_chain[0].recovered_block(), MIN_BLOCKS_FOR_PIPELINE_RUN + 10);

    let main_chain_last_hash = main_chain.last().unwrap().hash();
    test_harness.send_fcu(main_chain_last_hash, ForkchoiceStatus::Syncing).await;

    test_harness.check_fcu(main_chain_last_hash, ForkchoiceStatus::Syncing).await;

    // create event for backfill finished
    let backfill_finished_block_number = MIN_BLOCKS_FOR_PIPELINE_RUN + 1;
    let backfill_finished = FromOrchestrator::BackfillSyncFinished(ControlFlow::Continue {
        block_number: backfill_finished_block_number,
    });

    let backfill_tip_block = main_chain[(backfill_finished_block_number - 1) as usize].clone();
    // add block to mock provider to enable persistence clean up.
    test_harness.provider.add_block(backfill_tip_block.hash(), backfill_tip_block.into_block());
    test_harness.tree.on_engine_message(FromEngine::Event(backfill_finished)).unwrap();

    let event = test_harness.from_tree_rx.recv().await.unwrap();
    match event {
        EngineApiEvent::Download(DownloadRequest::BlockSet(hash_set)) => {
            assert_eq!(hash_set, HashSet::from_iter([main_chain_last_hash]));
        }
        _ => panic!("Unexpected event: {event:#?}"),
    }

    test_harness
        .tree
        .on_engine_message(FromEngine::DownloadedBlocks(vec![main_chain.last().unwrap().clone()]))
        .unwrap();

    let event = test_harness.from_tree_rx.recv().await.unwrap();
    match event {
        EngineApiEvent::Download(DownloadRequest::BlockRange(initial_hash, total_blocks)) => {
            assert_eq!(
                total_blocks,
                (main_chain.len() - backfill_finished_block_number as usize - 1) as u64
            );
            assert_eq!(initial_hash, main_chain.last().unwrap().parent_hash);
        }
        _ => panic!("Unexpected event: {event:#?}"),
    }
}
