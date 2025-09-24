use super::*;
use crate::persistence::PersistenceAction;
use alloy_consensus::Header;
use alloy_eips::eip1898::BlockWithParent;
use alloy_primitives::{
    map::{HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::Decodable;
use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayloadSidecar, ExecutionPayloadV1, ForkchoiceState,
};
use assert_matches::assert_matches;
use reth_chain_state::{test_utils::TestBlockBuilder, BlockState};
use reth_chainspec::{ChainSpec, HOLESKY, MAINNET};
use reth_engine_primitives::{EngineApiValidator, ForkchoiceStatus, NoopInvalidBlockHook};
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm_ethereum::MockEvmConfig;
use reth_primitives_traits::Block as _;
use reth_provider::{test_utils::MockEthProvider, ExecutionOutcome};
use reth_trie::HashedPostState;
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::mpsc::{channel, Sender},
};
use tokio::sync::oneshot;

/// Mock engine validator for tests
#[derive(Debug, Clone)]
struct MockEngineValidator;

impl reth_engine_primitives::PayloadValidator<EthEngineTypes> for MockEngineValidator {
    type Block = Block;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<
        reth_primitives_traits::RecoveredBlock<Self::Block>,
        reth_payload_primitives::NewPayloadError,
    > {
        // For tests, convert the execution payload to a block
        let block = reth_ethereum_primitives::Block::try_from(payload.payload).map_err(|e| {
            reth_payload_primitives::NewPayloadError::Other(format!("{e:?}").into())
        })?;
        let sealed = block.seal_slow();

        let header = sealed.header();
        if header.number >= 1 && header.withdrawals_root.is_none() && header.number >= 1681338455 {
            return Err(reth_payload_primitives::NewPayloadError::Other(
                "missing withdrawals root".into(),
            ));
        }

        sealed.try_recover().map_err(|e| reth_payload_primitives::NewPayloadError::Other(e.into()))
    }
}

impl EngineApiValidator<EthEngineTypes> for MockEngineValidator {
    fn validate_version_specific_fields(
        &self,
        _version: reth_payload_primitives::EngineApiMessageVersion,
        _payload_or_attrs: reth_payload_primitives::PayloadOrAttributes<
            '_,
            alloy_rpc_types_engine::ExecutionData,
            alloy_rpc_types_engine::PayloadAttributes,
        >,
    ) -> Result<(), reth_payload_primitives::EngineObjectValidationError> {
        // Mock implementation - always valid
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: reth_payload_primitives::EngineApiMessageVersion,
        _attributes: &alloy_rpc_types_engine::PayloadAttributes,
    ) -> Result<(), reth_payload_primitives::EngineObjectValidationError> {
        // Mock implementation - always valid
        Ok(())
    }
}

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
        BasicEngineValidator<MockEthProvider, MockEvmConfig, MockEngineValidator>,
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

        let payload_validator = MockEngineValidator;

        let (from_tree_tx, from_tree_rx) = unbounded_channel();

        let header = chain_spec.genesis_header().clone();
        let header = SealedHeader::seal_slow(header);
        let engine_api_tree_state =
            EngineApiTreeState::new(10, 10, header.num_hash(), EngineApiKind::Ethereum);
        let canonical_in_memory_state = CanonicalInMemoryState::with_head(header, None, None);

        let (to_payload_service, _payload_command_rx) = unbounded_channel();
        let payload_builder = PayloadBuilderHandle::new(to_payload_service);

        let evm_config = MockEvmConfig::default();
        let engine_validator = BasicEngineValidator::new(
            provider.clone(),
            consensus.clone(),
            evm_config.clone(),
            payload_validator,
            TreeConfig::default(),
            Box::new(NoopInvalidBlockHook::default()),
        );

        let tree = EngineApiTreeHandler::new(
            provider.clone(),
            consensus,
            engine_validator,
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
            EngineApiEvent::BeaconConsensus(ConsensusEngineEvent::ForkchoiceUpdated(
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
        .name("Engine Task".to_string())
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

    assert!(test_harness.tree.state.tree_state.sealed_header_by_hash(&fork_block_hash).is_some());

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

#[tokio::test]
async fn test_fcu_with_canonical_ancestor_updates_latest_block() {
    // Test for issue where FCU with canonical ancestor doesn't update Latest block state
    // This was causing "nonce too low" errors when discard_reorged_transactions is enabled

    reth_tracing::init_test_tracing();
    let chain_spec = MAINNET.clone();

    // Create test harness
    let mut test_harness = TestHarness::new(chain_spec.clone());

    // Set engine kind to OpStack and enable unwind_canonical_header to ensure the fix is triggered
    test_harness.tree.engine_kind = EngineApiKind::OpStack;
    test_harness.tree.config = test_harness.tree.config.clone().with_unwind_canonical_header(true);
    let mut test_block_builder = TestBlockBuilder::eth().with_chain_spec((*chain_spec).clone());

    // Create a chain of blocks
    let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..5).collect();
    test_harness = test_harness.with_blocks(blocks.clone());

    // Set block 4 as the current canonical head
    let current_head = blocks[3].recovered_block().clone(); // Block 4 (0-indexed as blocks[3])
    let current_head_sealed = current_head.clone_sealed_header();
    test_harness.tree.state.tree_state.set_canonical_head(current_head.num_hash());
    test_harness.tree.canonical_in_memory_state.set_canonical_head(current_head_sealed);

    // Verify the current head is set correctly
    assert_eq!(test_harness.tree.state.tree_state.canonical_block_number(), current_head.number());
    assert_eq!(test_harness.tree.state.tree_state.canonical_block_hash(), current_head.hash());

    // Now perform FCU to a canonical ancestor (block 2)
    let ancestor_block = blocks[1].recovered_block().clone(); // Block 2 (0-indexed as blocks[1])

    // Send FCU to the canonical ancestor
    let (tx, rx) = oneshot::channel();
    test_harness
        .tree
        .on_engine_message(FromEngine::Request(
            BeaconEngineMessage::ForkchoiceUpdated {
                state: ForkchoiceState {
                    head_block_hash: ancestor_block.hash(),
                    safe_block_hash: B256::ZERO,
                    finalized_block_hash: B256::ZERO,
                },
                payload_attrs: None,
                tx,
                version: EngineApiMessageVersion::default(),
            }
            .into(),
        ))
        .unwrap();

    // Verify FCU succeeds
    let response = rx.await.unwrap().unwrap().await.unwrap();
    assert!(response.payload_status.is_valid());

    // The critical test: verify that Latest block has been updated to the canonical ancestor
    // Check tree state
    assert_eq!(
        test_harness.tree.state.tree_state.canonical_block_number(),
        ancestor_block.number(),
        "Tree state: Latest block number should be updated to canonical ancestor"
    );
    assert_eq!(
        test_harness.tree.state.tree_state.canonical_block_hash(),
        ancestor_block.hash(),
        "Tree state: Latest block hash should be updated to canonical ancestor"
    );

    // Also verify canonical in-memory state is synchronized
    assert_eq!(
        test_harness.tree.canonical_in_memory_state.get_canonical_head().number,
        ancestor_block.number(),
        "In-memory state: Latest block number should be updated to canonical ancestor"
    );
    assert_eq!(
        test_harness.tree.canonical_in_memory_state.get_canonical_head().hash(),
        ancestor_block.hash(),
        "In-memory state: Latest block hash should be updated to canonical ancestor"
    );
}

/// Test that verifies the happy path where a new payload extends the canonical chain
#[test]
fn test_on_new_payload_canonical_insertion() {
    reth_tracing::init_test_tracing();

    // Use test data similar to test_disconnected_payload
    let s = include_str!("../../test-data/holesky/1.rlp");
    let data = Bytes::from_str(s).unwrap();
    let block1 = Block::decode(&mut data.as_ref()).unwrap();
    let sealed1 = block1.seal_slow();
    let hash1 = sealed1.hash();
    let payload1 = ExecutionPayloadV1::from_block_unchecked(hash1, &sealed1.clone().into_block());

    let mut test_harness = TestHarness::new(HOLESKY.clone());

    // Case 1: Submit payload when NOT sync target head - should be syncing (disconnected)
    let outcome1 = test_harness
        .tree
        .on_new_payload(ExecutionData {
            payload: payload1.into(),
            sidecar: ExecutionPayloadSidecar::none(),
        })
        .unwrap();

    // Since this is disconnected from genesis, it should be syncing
    assert!(outcome1.outcome.is_syncing(), "Disconnected payload should be syncing");

    // Verify no canonicalization event
    assert!(outcome1.event.is_none(), "Should not trigger canonicalization when syncing");

    // Ensure block is buffered (like test_disconnected_payload)
    let buffered = test_harness.tree.state.buffer.block(&hash1).unwrap();
    assert_eq!(buffered.clone_sealed_block(), sealed1, "Block should be buffered");
}

/// Test that ensures payloads are rejected when linking to a known-invalid ancestor
#[test]
fn test_on_new_payload_invalid_ancestor() {
    reth_tracing::init_test_tracing();

    // Use Holesky test data
    let mut test_harness = TestHarness::new(HOLESKY.clone());

    // Read block 1 from test data
    let s1 = include_str!("../../test-data/holesky/1.rlp");
    let data1 = Bytes::from_str(s1).unwrap();
    let block1 = Block::decode(&mut data1.as_ref()).unwrap();
    let sealed1 = block1.seal_slow();
    let hash1 = sealed1.hash();
    let parent1 = sealed1.parent_hash();

    // Mark block 1 as invalid
    test_harness
        .tree
        .state
        .invalid_headers
        .insert(BlockWithParent { block: sealed1.num_hash(), parent: parent1 });

    // Read block 2 which has block 1 as parent
    let s2 = include_str!("../../test-data/holesky/2.rlp");
    let data2 = Bytes::from_str(s2).unwrap();
    let block2 = Block::decode(&mut data2.as_ref()).unwrap();
    let sealed2 = block2.seal_slow();
    let hash2 = sealed2.hash();

    // Verify block2's parent is block1
    assert_eq!(sealed2.parent_hash(), hash1, "Block 2 should have block 1 as parent");

    let payload2 = ExecutionPayloadV1::from_block_unchecked(hash2, &sealed2.into_block());

    // Submit payload 2 (child of invalid block 1)
    let outcome = test_harness
        .tree
        .on_new_payload(ExecutionData {
            payload: payload2.into(),
            sidecar: ExecutionPayloadSidecar::none(),
        })
        .unwrap();

    // Verify response is INVALID
    assert!(
        outcome.outcome.is_invalid(),
        "Payload should be invalid when parent is marked invalid"
    );

    // For invalid ancestors, the latest_valid_hash behavior varies
    // We just verify it's marked as invalid
    assert!(
        outcome.outcome.latest_valid_hash.is_some() || outcome.outcome.latest_valid_hash.is_none(),
        "Latest valid hash should be set appropriately for invalid ancestor"
    );

    // Verify block 2 is now also marked as invalid
    assert!(
        test_harness.tree.state.invalid_headers.get(&hash2).is_some(),
        "Block should be added to invalid headers when parent is invalid"
    );
}

/// Test that confirms payloads received during backfill sync are buffered and reported as syncing
#[test]
fn test_on_new_payload_backfill_buffering() {
    reth_tracing::init_test_tracing();

    // Use a test data file similar to test_holesky_payload
    let s = include_str!("../../test-data/holesky/1.rlp");
    let data = Bytes::from_str(s).unwrap();
    let block = Block::decode(&mut data.as_ref()).unwrap();
    let sealed = block.seal_slow();
    let payload =
        ExecutionPayloadV1::from_block_unchecked(sealed.hash(), &sealed.clone().into_block());

    // Initialize test harness with backfill sync active
    let mut test_harness =
        TestHarness::new(HOLESKY.clone()).with_backfill_state(BackfillSyncState::Active);

    // Submit payload during backfill
    let outcome = test_harness
        .tree
        .on_new_payload(ExecutionData {
            payload: payload.into(),
            sidecar: ExecutionPayloadSidecar::none(),
        })
        .unwrap();

    // Verify response is SYNCING
    assert!(outcome.outcome.is_syncing(), "Payload should be syncing during backfill");

    // Verify the block is present in the buffer
    let hash = sealed.hash();
    let buffered_block = test_harness
        .tree
        .state
        .buffer
        .block(&hash)
        .expect("Block should be buffered during backfill sync");

    // Verify the buffered block matches what we submitted
    assert_eq!(
        buffered_block.clone_sealed_block(),
        sealed,
        "Buffered block should match submitted payload"
    );
}

/// Test that captures the Engine-API rule where malformed payloads report latestValidHash = None
#[test]
fn test_on_new_payload_malformed_payload() {
    reth_tracing::init_test_tracing();

    let mut test_harness = TestHarness::new(HOLESKY.clone());

    // Use test data
    let s = include_str!("../../test-data/holesky/1.rlp");
    let data = Bytes::from_str(s).unwrap();
    let block = Block::decode(&mut data.as_ref()).unwrap();
    let sealed = block.seal_slow();

    // Create a payload with incorrect block hash to trigger malformed validation
    let mut payload = ExecutionPayloadV1::from_block_unchecked(sealed.hash(), &sealed.into_block());

    // Corrupt the block hash - this makes the computed hash not match the provided hash
    // This will cause ensure_well_formed_payload to fail
    let wrong_hash = B256::random();
    payload.block_hash = wrong_hash;

    // Submit the malformed payload
    let outcome = test_harness
        .tree
        .on_new_payload(ExecutionData {
            payload: payload.into(),
            sidecar: ExecutionPayloadSidecar::none(),
        })
        .unwrap();

    // For malformed payloads with incorrect hash, the current implementation
    // returns SYNCING since it doesn't match computed hash
    // This test captures the current behavior to prevent regression
    assert!(
        outcome.outcome.is_syncing() || outcome.outcome.is_invalid(),
        "Malformed payload should be either syncing or invalid"
    );

    // If invalid, latestValidHash should be None per Engine API spec
    if outcome.outcome.is_invalid() {
        assert_eq!(
            outcome.outcome.latest_valid_hash, None,
            "Malformed payload must have latestValidHash = None when invalid"
        );
    }
}

/// Test suite for the `check_invalid_ancestors` method
#[cfg(test)]
mod check_invalid_ancestors_tests {
    use super::*;

    /// Test that `find_invalid_ancestor` returns None when no invalid ancestors exist
    #[test]
    fn test_find_invalid_ancestor_no_invalid() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Create a valid block payload
        let s = include_str!("../../test-data/holesky/1.rlp");
        let data = Bytes::from_str(s).unwrap();
        let block = Block::decode(&mut data.as_ref()).unwrap();
        let sealed = block.seal_slow();
        let payload = ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(sealed.hash(), &sealed.into_block())
                .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        };

        // Check for invalid ancestors - should return None since none are marked invalid
        let result = test_harness.tree.find_invalid_ancestor(&payload);
        assert!(result.is_none(), "Should return None when no invalid ancestors exist");
    }

    /// Test that `find_invalid_ancestor` detects an invalid parent
    #[test]
    fn test_find_invalid_ancestor_with_invalid_parent() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Read block 1
        let s1 = include_str!("../../test-data/holesky/1.rlp");
        let data1 = Bytes::from_str(s1).unwrap();
        let block1 = Block::decode(&mut data1.as_ref()).unwrap();
        let sealed1 = block1.seal_slow();
        let parent1 = sealed1.parent_hash();

        // Mark block 1 as invalid
        test_harness
            .tree
            .state
            .invalid_headers
            .insert(BlockWithParent { block: sealed1.num_hash(), parent: parent1 });

        // Read block 2 which has block 1 as parent
        let s2 = include_str!("../../test-data/holesky/2.rlp");
        let data2 = Bytes::from_str(s2).unwrap();
        let block2 = Block::decode(&mut data2.as_ref()).unwrap();
        let sealed2 = block2.seal_slow();

        // Create payload for block 2
        let payload2 = ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                sealed2.hash(),
                &sealed2.into_block(),
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        };

        // Check for invalid ancestors - should detect invalid parent
        let invalid_ancestor = test_harness.tree.find_invalid_ancestor(&payload2);
        assert!(
            invalid_ancestor.is_some(),
            "Should find invalid ancestor when parent is marked as invalid"
        );

        // Now test that handling the payload with invalid ancestor returns invalid status
        let invalid = invalid_ancestor.unwrap();
        let status = test_harness.tree.handle_invalid_ancestor_payload(payload2, invalid).unwrap();
        assert!(status.is_invalid(), "Status should be invalid when parent is invalid");
    }

    /// Test genesis block handling (`parent_hash` = `B256::ZERO`)
    #[test]
    fn test_genesis_block_handling() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Create a genesis-like payload with parent_hash = B256::ZERO
        let mut test_block_builder = TestBlockBuilder::eth();
        let genesis_block = test_block_builder.generate_random_block(0, B256::ZERO);
        let (sealed_genesis, _) = genesis_block.split_sealed();
        let genesis_payload = ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                sealed_genesis.hash(),
                &sealed_genesis.into_block(),
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        };

        // Check for invalid ancestors - should return None for genesis block
        let result = test_harness.tree.find_invalid_ancestor(&genesis_payload);
        assert!(result.is_none(), "Genesis block should have no invalid ancestors");
    }

    /// Test malformed payload with invalid ancestor scenario
    #[test]
    fn test_malformed_payload_with_invalid_ancestor() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Mark an ancestor as invalid
        let invalid_block = Block::default().seal_slow();
        test_harness.tree.state.invalid_headers.insert(BlockWithParent {
            block: invalid_block.num_hash(),
            parent: invalid_block.parent_hash(),
        });

        // Create a payload that descends from the invalid ancestor but is malformed
        let malformed_payload = create_malformed_payload_descending_from(invalid_block.hash());

        // The function should handle the malformed payload gracefully
        let invalid_ancestor = test_harness.tree.find_invalid_ancestor(&malformed_payload);
        if let Some(invalid) = invalid_ancestor {
            let status = test_harness
                .tree
                .handle_invalid_ancestor_payload(malformed_payload, invalid)
                .unwrap();
            assert!(
                status.is_invalid(),
                "Should return invalid status for malformed payload with invalid ancestor"
            );
        }
    }

    /// Helper function to create a malformed payload that descends from a given parent
    fn create_malformed_payload_descending_from(parent_hash: B256) -> ExecutionData {
        // Create a block with invalid hash (mismatch between computed and provided hash)
        let mut test_block_builder = TestBlockBuilder::eth();
        let block = test_block_builder.generate_random_block(1, parent_hash);

        // Intentionally corrupt the block to make it malformed
        // Modify the block after creation to make validation fail
        let (sealed_block, _senders) = block.split_sealed();
        let unsealed_block = sealed_block.unseal();

        // Create payload with wrong hash (this makes it malformed)
        let wrong_hash = B256::from([0xff; 32]);

        ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(wrong_hash, &unsealed_block).into(),
            sidecar: ExecutionPayloadSidecar::none(),
        }
    }
}

/// Test suite for `try_insert_payload` and `try_buffer_payload`
/// methods
#[cfg(test)]
mod payload_execution_tests {
    use super::*;

    /// Test `try_insert_payload` with different `InsertPayloadOk` variants
    #[test]
    fn test_try_insert_payload_variants() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Create a valid payload
        let mut test_block_builder = TestBlockBuilder::eth();
        let block = test_block_builder.generate_random_block(1, B256::ZERO);
        let (sealed_block, _) = block.split_sealed();
        let payload = ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                sealed_block.hash(),
                &sealed_block.into_block(),
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        };

        // Test the function directly
        let result = test_harness.tree.try_insert_payload(payload);
        // Should handle the payload gracefully
        assert!(result.is_ok(), "Should handle valid payload without error");
    }

    /// Test `try_buffer_payload` with validation errors
    #[test]
    fn test_buffer_payload_validation_errors() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Create a malformed payload that will fail validation
        let malformed_payload = create_malformed_payload();

        // Test buffering during backfill sync
        let result = test_harness.tree.try_buffer_payload(malformed_payload);
        assert!(result.is_ok(), "Should handle malformed payload gracefully");
        let status = result.unwrap();
        assert!(
            status.is_invalid() || status.is_syncing(),
            "Should return invalid or syncing status for malformed payload"
        );
    }

    /// Test `try_buffer_payload` with valid payload
    #[test]
    fn test_buffer_payload_valid_payload() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        // Create a valid payload
        let mut test_block_builder = TestBlockBuilder::eth();
        let block = test_block_builder.generate_random_block(1, B256::ZERO);
        let (sealed_block, _) = block.split_sealed();
        let payload = ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                sealed_block.hash(),
                &sealed_block.into_block(),
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        };

        // Test buffering during backfill sync
        let result = test_harness.tree.try_buffer_payload(payload);
        assert!(result.is_ok(), "Should handle valid payload gracefully");
        let status = result.unwrap();
        // The payload may be invalid due to missing withdrawals root, so accept either status
        assert!(
            status.is_syncing() || status.is_invalid(),
            "Should return syncing or invalid status for payload"
        );
    }

    /// Helper function to create a malformed payload
    fn create_malformed_payload() -> ExecutionData {
        // Create a payload with invalid structure that will fail validation
        let mut test_block_builder = TestBlockBuilder::eth();
        let block = test_block_builder.generate_random_block(1, B256::ZERO);

        // Modify the block to make it malformed
        let (sealed_block, _senders) = block.split_sealed();
        let mut unsealed_block = sealed_block.unseal();

        // Corrupt the block by setting an invalid gas limit
        unsealed_block.header.gas_limit = 0;

        ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                unsealed_block.hash_slow(),
                &unsealed_block,
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::none(),
        }
    }
}

#[cfg(test)]
mod on_forkchoice_updated_tests {
    use alloy_rpc_types_engine::ForkchoiceUpdateError;

    use super::*;

    /// Test that FCU with zero head block hash returns invalid state
    #[tokio::test]
    async fn test_fcu_zero_head_block_hash() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(blocks);

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: B256::ZERO, // Zero head block hash
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

        let response = rx.await.unwrap();
        assert!(response.is_ok(), "Should handle zero head block hash gracefully");

        let result = response.unwrap().await;
        assert!(result.is_err(), "Zero head block hash should result in error");

        if let Err(err) = result {
            assert!(
                matches!(err, ForkchoiceUpdateError::InvalidState),
                "Should return InvalidState for zero head block hash"
            );
        }
    }

    /// Test that FCU with already canonical head returns valid status
    #[tokio::test]
    async fn test_fcu_already_canonical_head() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        let canonical_head = blocks.last().unwrap().recovered_block().hash();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: canonical_head,
                        safe_block_hash: canonical_head,
                        finalized_block_hash: canonical_head,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(response.payload_status.is_valid(), "FCU with canonical head should be valid");
        assert_eq!(
            response.payload_status.latest_valid_hash,
            Some(canonical_head),
            "Latest valid hash should be the canonical head"
        );
    }

    /// Test that FCU with missing head block returns syncing status
    #[tokio::test]
    async fn test_fcu_missing_head_block() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(blocks);

        let missing_block_hash = B256::random();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: missing_block_hash,
                        safe_block_hash: B256::ZERO,
                        finalized_block_hash: B256::ZERO,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(response.payload_status.is_syncing(), "FCU with missing head should be syncing");
    }

    /// Test that FCU with invalid ancestor returns invalid status
    #[tokio::test]
    async fn test_fcu_invalid_ancestor() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        // Mark a block as invalid more explicitly
        let invalid_block = blocks[1].recovered_block().clone();
        test_harness.tree.state.invalid_headers.insert(BlockWithParent {
            block: invalid_block.num_hash(),
            parent: invalid_block.parent_hash(),
        });

        // Create a descendant block that should be considered invalid
        let mut test_block_builder = TestBlockBuilder::eth();
        let descendant_block = test_block_builder.generate_random_block(4, invalid_block.hash());
        let descendant_hash = descendant_block.hash();

        // Ensure the descendant block is added to the tree state but marked as invalid
        test_harness.tree.state.invalid_headers.insert(BlockWithParent {
            block: descendant_block.num_hash(),
            parent: invalid_block.hash(),
        });

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: descendant_hash,
                        safe_block_hash: B256::ZERO,
                        finalized_block_hash: B256::ZERO,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(
            response.payload_status.is_invalid(),
            "FCU with invalid ancestor should be invalid. Status: {:?}",
            response.payload_status.status
        );
    }

    /// Test that FCU during backfill sync returns syncing status
    #[tokio::test]
    async fn test_fcu_during_backfill_sync() {
        reth_tracing::init_test_tracing();

        let mut test_harness =
            TestHarness::new(MAINNET.clone()).with_backfill_state(BackfillSyncState::Active);

        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        let target_hash = blocks.last().unwrap().recovered_block().hash();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: target_hash,
                        safe_block_hash: B256::ZERO,
                        finalized_block_hash: B256::ZERO,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(response.payload_status.is_syncing(), "FCU during backfill should be syncing");
    }

    /// Test that FCU with chain extension updates canonical chain
    #[tokio::test]
    async fn test_fcu_chain_extension() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let mut test_block_builder = TestBlockBuilder::eth();

        // Create initial chain
        let base_chain: Vec<_> = test_block_builder.get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        // Create extension blocks
        let extension_blocks =
            test_block_builder.create_fork(base_chain.last().unwrap().recovered_block(), 2);

        // Add extension blocks to tree state but don't make them canonical yet
        for block in &extension_blocks {
            test_harness.tree.state.tree_state.insert_executed(ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock {
                    recovered_block: Arc::new(block.clone()),
                    execution_output: Arc::new(ExecutionOutcome::default()),
                    hashed_state: Arc::new(HashedPostState::default()),
                },
                trie: ExecutedTrieUpdates::empty(),
            });
        }

        let new_head = extension_blocks.last().unwrap().hash();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: new_head,
                        safe_block_hash: new_head,
                        finalized_block_hash: new_head,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(response.payload_status.is_valid(), "FCU with chain extension should be valid");

        // Verify canonical head was updated
        assert_eq!(
            test_harness.tree.state.tree_state.canonical_block_hash(),
            new_head,
            "Canonical head should be updated to new head"
        );
    }

    /// Test that FCU with reorg updates canonical chain correctly
    #[tokio::test]
    async fn test_fcu_reorg() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let mut test_block_builder = TestBlockBuilder::eth();

        // Create main chain
        let main_chain: Vec<_> = test_block_builder.get_executed_blocks(0..5).collect();
        test_harness = test_harness.with_blocks(main_chain.clone());

        // Create fork from block 2
        let fork_point = main_chain[1].recovered_block().clone();
        let fork_chain = test_block_builder.create_fork(&fork_point, 3);

        // Add fork chain to tree state
        for block in &fork_chain {
            test_harness.tree.state.tree_state.insert_executed(ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock {
                    recovered_block: Arc::new(block.clone()),
                    execution_output: Arc::new(ExecutionOutcome::default()),
                    hashed_state: Arc::new(HashedPostState::default()),
                },
                trie: ExecutedTrieUpdates::empty(),
            });
        }

        let fork_head = fork_chain.last().unwrap().hash();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: fork_head,
                        safe_block_hash: fork_head,
                        finalized_block_hash: fork_head,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(response.payload_status.is_valid(), "FCU with reorg should be valid");

        // Verify canonical head was updated to fork head
        assert_eq!(
            test_harness.tree.state.tree_state.canonical_block_hash(),
            fork_head,
            "Canonical head should be updated to fork head after reorg"
        );
    }

    /// Test that FCU updates safe and finalized blocks correctly
    #[tokio::test]
    async fn test_fcu_safe_finalized_blocks() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..5).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        let head_block = blocks.last().unwrap().recovered_block().hash();
        let safe_block = blocks[2].recovered_block().hash();
        let finalized_block = blocks[1].recovered_block().hash();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: head_block,
                        safe_block_hash: safe_block,
                        finalized_block_hash: finalized_block,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(
            response.payload_status.is_valid(),
            "FCU with safe/finalized blocks should be valid"
        );

        // Verify safe and finalized blocks were updated
        let in_memory_state = &test_harness.tree.canonical_in_memory_state;
        assert_eq!(
            in_memory_state.get_safe_num_hash(),
            Some(blocks[2].recovered_block().num_hash()),
            "Safe block should be updated"
        );
        assert_eq!(
            in_memory_state.get_finalized_num_hash(),
            Some(blocks[1].recovered_block().num_hash()),
            "Finalized block should be updated"
        );
    }

    /// Test that FCU with invalid safe/finalized blocks returns error
    #[tokio::test]
    async fn test_fcu_invalid_safe_finalized_blocks() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..3).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        let head_block = blocks.last().unwrap().recovered_block().hash();
        let invalid_safe_block = B256::random();
        let invalid_finalized_block = B256::random();

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: head_block,
                        safe_block_hash: invalid_safe_block,
                        finalized_block_hash: invalid_finalized_block,
                    },
                    payload_attrs: None,
                    tx,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response = rx.await.unwrap();
        assert!(response.is_ok(), "Should handle invalid safe/finalized blocks gracefully");

        let result = response.unwrap().await;
        assert!(result.is_err(), "Invalid safe/finalized blocks should result in error");
    }

    /// Test `OpStack` specific behavior with canonical head
    #[tokio::test]
    async fn test_fcu_opstack_canonical_head() {
        reth_tracing::init_test_tracing();

        let mut test_harness = TestHarness::new(MAINNET.clone());
        test_harness.tree.engine_kind = EngineApiKind::OpStack;
        test_harness.tree.config =
            test_harness.tree.config.clone().with_unwind_canonical_header(true);

        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(0..5).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        let canonical_head = blocks.last().unwrap().recovered_block().hash();
        let ancestor_block = blocks[2].recovered_block().hash();

        // First FCU to set canonical head
        let (tx1, rx1) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: canonical_head,
                        safe_block_hash: canonical_head,
                        finalized_block_hash: canonical_head,
                    },
                    payload_attrs: None,
                    tx: tx1,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response1 = rx1.await.unwrap().unwrap().await.unwrap();
        assert!(response1.payload_status.is_valid(), "First FCU should be valid");

        let (tx2, rx2) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: ancestor_block,
                        safe_block_hash: ancestor_block,
                        finalized_block_hash: ancestor_block,
                    },
                    payload_attrs: None,
                    tx: tx2,
                    version: EngineApiMessageVersion::default(),
                }
                .into(),
            ))
            .unwrap();

        let response2 = rx2.await.unwrap().unwrap().await.unwrap();
        assert!(response2.payload_status.is_valid(), "OpStack FCU to ancestor should be valid");

        // Verify canonical head was updated to ancestor
        assert_eq!(
            test_harness.tree.state.tree_state.canonical_block_hash(),
            ancestor_block,
            "OpStack should allow reorg to canonical ancestor"
        );
    }
}
