//! A seeded node-core integration: real transactions, payload construction, EVM validation,
//! forkchoice, live block download, persistence, and engine restart.
//!
//! Native database operations and synchronous EVM execution are atomic simulation steps. The
//! production node launcher, RPC sockets, discovery, and parallel execution workers are outside
//! this profile. No pre-executed blocks are inserted into either engine.

use super::{
    node_storage::{Factory, NodeStorage, NodeTypes},
    node_wire::{WireBlockClient, WireEvent, WirePeer},
    *,
};
use crate::{
    chain::{ChainHandler, FromOrchestrator, HandlerEvent},
    download::BasicBlockDownloader,
    engine::{EngineApiRequestHandler, EngineHandler},
    persistence::PersistenceError,
};
use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_primitives::{Address, U256};
use alloy_signer::SignerSync;
use commonware_runtime::{deterministic, Runner, Supervisor};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpecBuilder, ChainSpecProvider};
use reth_db_common::init::init_genesis_with_settings;
use reth_e2e_test_utils::wallet::Wallet;
use reth_eth_wire::simulation::LinkConfig;
use reth_ethereum_engine_primitives::EthBuiltPayload;
use reth_ethereum_payload_builder::{EthereumBuilderConfig, EthereumPayloadBuilder};
use reth_ethereum_primitives::TransactionSigned;
use reth_evm_ethereum::EthEvmConfig;
use reth_exex_types::FinishedExExHeight;
use reth_node_ethereum::EthereumEngineValidator;
use reth_payload_builder::PayloadBuilderService;
use reth_payload_primitives::PayloadKind;
use reth_primitives_traits::SignerRecoverable;
use reth_provider::{
    providers::BlockchainProvider, AccountReader, BlockHashReader, BlockNumReader, StorageSettings,
};
use reth_prune::Pruner;
use reth_tasks::{TaskHandle, TaskRuntime};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore,
    validate::{EthTransactionValidator, EthTransactionValidatorBuilder},
    BestTransactions, BestTransactionsAttributes, CoinbaseTipOrdering, EthPooledTransaction, Pool,
    PoolTransaction, TransactionOrigin, TransactionPool, TransactionPoolExt,
};
use std::sync::{atomic::AtomicUsize, Mutex};
use tokio_stream::wrappers::UnboundedReceiverStream;

struct Node {
    provider: Provider,
    overlay: OverlayManager,
    pool: TxPool,
    tasks: TaskRuntime,
    prewarming: Arc<AtomicUsize>,
    txpool_snapshot: Box<dyn Fn(B256) -> Option<TxPoolPrewarmCacheSnapshot> + Send + Sync>,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    payload_service: TaskHandle<()>,
    stop_payload_service: oneshot::Sender<()>,
    input: tokio::sync::mpsc::UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    to_tree: crossbeam_channel::Sender<
        FromEngine<EngineApiRequest<EthEngineTypes, EthPrimitives>, Block>,
    >,
    engine: TaskHandle<()>,
    router: TaskHandle<()>,
    stop_router: oneshot::Sender<()>,
    persistence: TaskHandle<Result<(), PersistenceError>>,
    peer: WirePeer,
}

impl Node {
    async fn launch(
        factory: Factory,
        overlay: OverlayManager,
        blocks: Arc<Mutex<BTreeMap<u64, SealedBlock<Block>>>>,
        tasks: TaskRuntime,
        native: reth_tasks::Runtime,
        seed: u64,
    ) -> Self {
        let provider = BlockchainProvider::new(factory.clone()).unwrap();
        let chain = provider.chain_spec();
        let evm = EthEvmConfig::new(chain.clone());
        let consensus = Arc::new(EthBeaconConsensus::new(chain.clone()));
        let config = TreeConfig::default()
            .with_cross_block_cache_size(1024 * 1024)
            .with_memory_block_buffer_target(0)
            .with_persistence_threshold(2)
            .with_persistence_backpressure_threshold(4)
            .with_num_state_masking_blocks(1);
        let (_, exex) = tokio::sync::watch::channel(FinishedExExHeight::NoExExs);
        let pruner = Pruner::new_with_factory(factory.clone(), vec![], 5, 0, None, exex);
        let (metrics, _) = unbounded_channel();
        let (persistence, persistence_task) =
            PersistenceHandle::<EthPrimitives>::spawn_service_with_runtime(
                factory,
                pruner,
                metrics,
                tasks.clone(),
            );
        let blob_store = InMemoryBlobStore::default();
        let pool = Pool::new(
            EthTransactionValidatorBuilder::new(provider.clone(), evm.clone())
                .build(blob_store.clone()),
            CoinbaseTipOrdering::default(),
            blob_store,
            Default::default(),
        );
        let validator = BasicEngineValidator::new(
            provider.clone(),
            consensus.clone(),
            evm.clone(),
            EthereumEngineValidator::new(chain),
            config.clone(),
            Box::new(NoopInvalidBlockHook::default()),
            overlay.clone(),
            native.clone(),
        )
        .with_cooperative_sparse_trie(tasks.clone())
        .with_cooperative_prewarming()
        .with_txpool_prewarming(PoolPrewarmSource(pool.clone()));
        let prewarming = validator.prewarming_counter();
        let txpool_snapshot = validator.txpool_snapshot_observer();
        let payload_builder = EthereumPayloadBuilder::new(
            provider.clone(),
            pool.clone(),
            evm.clone(),
            EthereumBuilderConfig::default(),
        );
        let generator = BasicPayloadJobGenerator::with_builder(
            provider.clone(),
            native.clone(),
            BasicPayloadJobGeneratorConfig::default().interval(Duration::from_millis(10)),
            payload_builder,
        )
        .with_task_runtime(tasks.clone());
        let (payload_service, payload_builder) = PayloadBuilderService::<_, _, EthEngineTypes>::new(
            generator,
            provider.canonical_state_stream(),
        );
        let (stop_payload_service, stopped_payload_service) = oneshot::channel();
        let payload_service = tasks.spawn("payload_service", async move {
            tokio::select! {
                biased;
                _ = stopped_payload_service => {}
                _ = payload_service => {}
            }
        });
        let (tree, events) = EngineApiTreeHandler::new_from_provider(
            provider.clone(),
            consensus.clone(),
            validator,
            persistence,
            payload_builder.clone(),
            provider.canonical_in_memory_state(),
            overlay.clone(),
            config,
            EngineApiKind::Ethereum,
            evm.clone(),
            native,
        );
        let to_tree = tree.sender();
        let engine_runtime = tasks.clone();
        let engine = tasks.spawn("engine", tree.run_cooperative(engine_runtime));
        let (client, peer) = WireBlockClient::new(
            tasks.clone(),
            blocks,
            LinkConfig {
                seed,
                capacity: 512,
                max_chunk: 71,
                latency: Duration::from_millis(1),
                jitter: Duration::from_millis(2),
            },
        )
        .await;
        let (input, incoming) = unbounded_channel();
        let mut handler = EngineHandler::new(
            EngineApiRequestHandler::new(to_tree.clone(), events),
            BasicBlockDownloader::new(client, consensus),
            UnboundedReceiverStream::new(incoming),
        );
        let (stop_router, mut stopped_router) = oneshot::channel();
        let router = tasks.spawn("router", async move {
            loop {
                let event = tokio::select! {
                    biased;
                    _ = &mut stopped_router => return,
                    event = futures::future::poll_fn(|cx| handler.poll(cx)) => event,
                };
                match event {
                    HandlerEvent::Event(_) => {}
                    HandlerEvent::BackfillAction(action) => {
                        panic!("short live-sync scenario requested backfill: {action:?}")
                    }
                    HandlerEvent::FatalError => panic!("engine router failed"),
                }
            }
        });
        Self {
            provider,
            overlay,
            pool,
            tasks,
            prewarming,
            txpool_snapshot,
            payload_builder,
            payload_service,
            stop_payload_service,
            input,
            to_tree,
            engine,
            router,
            stop_router,
            persistence: persistence_task,
            peer,
        }
    }

    async fn build(&self, parent: &SealedHeader, nonce: u64, branch: u8) -> EthBuiltPayload {
        // The harness supplies pool head maintenance while the production maintenance actor
        // remains outside this profile. The prewarmer uses the same live best-tx iterator.
        let mut info = self.pool.block_info();
        info.last_seen_block_hash = parent.hash();
        info.last_seen_block_number = parent.number;
        info.block_gas_limit = parent.gas_limit;
        self.pool.set_block_info(info);
        let wallet = Wallet::default();
        let mut hashes = Vec::new();
        for nonce in nonce..nonce + TRANSACTIONS_PER_BLOCK {
            let transaction = TxEip1559 {
                chain_id: 1,
                nonce,
                gas_limit: 21_000,
                max_fee_per_gas: 1_000_000_000_000,
                max_priority_fee_per_gas: 20_000_000_000,
                to: Address::repeat_byte(0x42).into(),
                value: U256::from(100),
                ..Default::default()
            };
            let signature = wallet.inner.sign_hash_sync(&transaction.signature_hash()).unwrap();
            let transaction = TransactionSigned::from(transaction.into_signed(signature));
            let transaction =
                EthPooledTransaction::try_from_consensus(transaction.try_into_recovered().unwrap())
                    .unwrap();
            let hash =
                self.pool.add_transaction(TransactionOrigin::Local, transaction).await.unwrap();
            hashes.push(hash.hash);
        }
        // Let the persistent worker publish actual state reads before the payload-build lease
        // pauses it. A snapshot contains parent state, never speculative execution writes.
        assert!(self.forkchoice(parent.hash()).await.is_valid());
        let deadline = self.tasks.now() + Duration::from_secs(1);
        let sender = wallet.inner.address();
        loop {
            if let Some(snapshot) = (self.txpool_snapshot)(parent.hash()) {
                assert_eq!(snapshot.account(&sender).unwrap().unwrap().nonce, nonce);
                assert!(snapshot.entry_counts().0 >= 2);
                break;
            }
            assert!(self.tasks.now() < deadline, "txpool prewarming did not publish a snapshot");
            self.tasks.sleep(Duration::from_millis(1)).await;
        }
        let attributes = EthPayloadAttributes {
            timestamp: parent.timestamp + 12 + u64::from(branch),
            prev_randao: B256::repeat_byte(branch),
            suggested_fee_recipient: Address::repeat_byte(branch + 1),
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::repeat_byte(branch)),
            ..Default::default()
        };
        let (tx, rx) = oneshot::channel();
        self.input
            .send(BeaconEngineMessage::ForkchoiceUpdated {
                state: ForkchoiceState {
                    head_block_hash: parent.hash(),
                    safe_block_hash: B256::ZERO,
                    finalized_block_hash: B256::ZERO,
                },
                payload_attrs: Some(attributes),
                tx,
            })
            .unwrap();
        let response = rx.await.unwrap().unwrap().await.unwrap();
        assert!(response.payload_status.is_valid());
        let payload = self
            .payload_builder
            .resolve_kind(
                response.payload_id.expect("FCU started payload job"),
                PayloadKind::WaitForPending,
            )
            .await
            .unwrap()
            .unwrap();
        self.pool.remove_transactions(hashes);
        assert_eq!(payload.block().body().transactions.len() as u64, TRANSACTIONS_PER_BLOCK);
        assert_eq!(payload.block().gas_used(), 21_000 * TRANSACTIONS_PER_BLOCK);
        payload
    }

    async fn new_payload(&self, payload: ExecutionData) -> PayloadStatus {
        let (tx, rx) = oneshot::channel();
        self.input.send(BeaconEngineMessage::NewPayload { payload, tx }).unwrap();
        rx.await.unwrap().unwrap()
    }

    async fn forkchoice(&self, head: B256) -> PayloadStatus {
        let (tx, rx) = oneshot::channel();
        self.input
            .send(BeaconEngineMessage::ForkchoiceUpdated {
                state: ForkchoiceState {
                    head_block_hash: head,
                    safe_block_hash: B256::ZERO,
                    finalized_block_hash: B256::ZERO,
                },
                payload_attrs: None,
                tx,
            })
            .unwrap();
        rx.await.unwrap().unwrap().await.unwrap().payload_status
    }

    async fn import(&self, payload: &EthBuiltPayload) {
        assert!(self.new_payload(payload.clone().into()).await.is_valid());
        self.assert_sparse_root(payload.block().state_root());
        assert!(self.forkchoice(payload.block().hash()).await.is_valid());
    }

    fn assert_sparse_root(&self, state_root: B256) {
        let trie =
            self.overlay.take_sparse_trie().expect("validation must preserve its sparse trie");
        assert_eq!(trie.state_root(), state_root);
        self.overlay.store_sparse_trie(trie);
    }

    async fn shutdown(self) {
        self.stop_router.send(()).unwrap();
        self.router.await.unwrap();
        self.stop_payload_service.send(()).unwrap();
        self.payload_service.await.unwrap();
        drop(self.payload_builder);
        let (tx, rx) = oneshot::channel();
        self.to_tree.send(FromEngine::Event(FromOrchestrator::Terminate { tx })).unwrap();
        rx.await.unwrap();
        self.engine.await.unwrap();
        self.persistence.await.unwrap().unwrap();
        self.peer.shutdown().await;
    }
}

type Provider = BlockchainProvider<NodeTypes>;
type TxPool = Pool<
    EthTransactionValidator<Provider, EthPooledTransaction, EthEvmConfig>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    InMemoryBlobStore,
>;

const TRANSACTIONS_PER_BLOCK: u64 = payload_processor::SMALL_BLOCK_TX_THRESHOLD as u64;

#[derive(Debug)]
struct PoolPrewarmSource(TxPool);

impl TxPoolPrewarmSource<EthPrimitives> for PoolPrewarmSource {
    fn best_transactions(
        &self,
        parent_hash: B256,
    ) -> Option<TxPoolPrewarmTransactions<EthPrimitives>> {
        let info = self.0.block_info();
        if info.last_seen_block_hash != parent_hash {
            return None;
        }
        let mut best = self.0.best_transactions_with_attributes(BestTransactionsAttributes::new(
            info.pending_basefee,
            info.pending_blob_fee.map(|fee| u64::try_from(fee).unwrap_or(u64::MAX)),
        ));
        best.allow_updates_out_of_order();
        best.skip_blobs();
        Some(Box::new(best.map(|transaction| TxPoolPrewarmTransaction {
            hash: *transaction.hash(),
            sender: transaction.sender(),
            transaction: transaction.transaction.clone_into_consensus(),
        })))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NodeOutcome {
    audit: String,
    canonical: Vec<B256>,
    first_block: SealedBlock<Block>,
    wire: Vec<WireEvent>,
    persisted_head: B256,
    sender_nonce: u64,
    sender_balance: U256,
    prewarmed_transactions: [usize; 3],
}

fn node_chain() -> Arc<ChainSpec> {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(
                serde_json::from_str(include_str!(
                    "../../../../../e2e-test-utils/src/testsuite/assets/genesis.json"
                ))
                .unwrap(),
            )
            .cancun_activated()
            .build(),
    )
}

fn simulate_node(seed: u64) -> NodeOutcome {
    let chain = node_chain();
    let native = reth_tasks::Runtime::test();
    let mut producer_storage = NodeStorage::new(chain.clone());
    let mut follower_storage = NodeStorage::new(chain.clone());
    let producer_overlay = OverlayManager::default();
    let producer_factory = producer_storage.open(producer_overlay.clone(), native.clone());
    let follower_overlay = OverlayManager::default();
    let follower_factory = follower_storage.open(follower_overlay.clone(), native.clone());
    init_genesis_with_settings(&producer_factory, StorageSettings::v2()).unwrap();
    init_genesis_with_settings(&follower_factory, StorageSettings::v2()).unwrap();
    let genesis_block = producer_factory.block_by_number(0).unwrap().unwrap().seal_slow();
    let config = deterministic::Config::default()
        .with_seed(seed)
        // Several worker polls must fit inside the validator's 1ms speculative window.
        .with_cycle(Duration::from_micros(10))
        .with_timeout(Some(Duration::from_secs(30)));
    let outcome = deterministic::Runner::new(config).start(|context| async move {
        let tasks = TaskRuntime::deterministic(context.child("nodes"));
        let blocks = Arc::new(Mutex::new(BTreeMap::from([(0, genesis_block)])));
        let producer = Node::launch(
            producer_factory,
            producer_overlay,
            blocks.clone(),
            TaskRuntime::deterministic(context.child("producer")),
            native.clone(),
            seed,
        )
        .await;
        let follower = Node::launch(
            follower_factory,
            follower_overlay,
            blocks.clone(),
            TaskRuntime::deterministic(context.child("follower")),
            native.clone(),
            seed.wrapping_add(1),
        )
        .await;
        let genesis = SealedHeader::seal_slow(chain.genesis_header().clone());
        let one = producer.build(&genesis, 0, 0).await;
        producer.import(&one).await;
        // Build both candidates while the real pool still validates the next nonces against their
        // common parent; then switch the canonical head from the first candidate to the second.
        let abandoned =
            producer.build(one.block().sealed_header(), TRANSACTIONS_PER_BLOCK, 0).await;
        let two = producer.build(one.block().sealed_header(), TRANSACTIONS_PER_BLOCK, 1).await;
        producer.import(&abandoned).await;
        producer.import(&two).await;
        assert_ne!(two.block().hash(), abandoned.block().hash());
        assert_ne!(two.block().state_root(), abandoned.block().state_root());
        let three =
            producer.build(two.block().sealed_header(), 2 * TRANSACTIONS_PER_BLOCK, 1).await;
        producer.import(&three).await;
        let four =
            producer.build(three.block().sealed_header(), 3 * TRANSACTIONS_PER_BLOCK, 1).await;
        // Rehash each invalid header so rejection reaches EVM receipt/state-root validation.
        for corrupt_state in [false, true] {
            let mut invalid_block = four.block().clone().into_block();
            if corrupt_state {
                invalid_block.header.state_root = B256::repeat_byte(0xa5);
            } else {
                invalid_block.header.receipts_root = B256::repeat_byte(0x5a);
            }
            let (payload, sidecar) =
                alloy_rpc_types_engine::ExecutionPayload::from_block_slow(&invalid_block);
            let status = producer.new_payload(ExecutionData::new(payload, sidecar)).await;
            assert!(status.is_invalid(), "corrupt_state={corrupt_state}: {status:?}");
            let expected =
                if corrupt_state { "mismatched block state root" } else { "receipt root mismatch" };
            assert!(
                status.status.validation_error().unwrap_or_default().contains(expected),
                "{status:?}"
            );
        }
        producer.import(&four).await;
        for payload in [&one, &two, &three, &four] {
            blocks.lock().unwrap().insert(payload.block().number(), payload.block().clone());
        }
        // The follower knows only genesis. Its real downloader fetches the missing ancestry over
        // fragmented ETH protocol frames after forkchoice names an unknown head.
        assert!(follower.forkchoice(four.block().hash()).await.is_syncing());
        loop {
            tasks.sleep(Duration::from_millis(5)).await;
            if follower.forkchoice(four.block().hash()).await.is_valid() {
                break;
            }
        }
        let canonical: Vec<_> =
            [&one, &two, &three, &four].into_iter().map(|p| p.block().hash()).collect();
        assert_eq!(producer.provider.best_block_number().unwrap(), 4);
        assert_eq!(follower.provider.best_block_number().unwrap(), 4);
        follower.assert_sparse_root(four.block().state_root());
        for (index, hash) in canonical.iter().enumerate() {
            assert_eq!(follower.provider.block_hash(index as u64 + 1).unwrap(), Some(*hash));
        }
        // Observe a real threshold-triggered write before graceful shutdown flushes the tail.
        loop {
            let producer_tip =
                producer.provider.database_provider_ro().unwrap().best_block_number().unwrap();
            let follower_tip =
                follower.provider.database_provider_ro().unwrap().best_block_number().unwrap();
            if producer_tip >= 3 && follower_tip >= 3 {
                break;
            }
            tasks.sleep(Duration::from_millis(1)).await;
        }
        let sender = Address::from_str("f39fd6e51aad88f6f4ce6ab8827279cfffb92266").unwrap();
        let producer_account = producer.provider.latest().unwrap().basic_account(&sender).unwrap();
        let follower_account = follower.provider.latest().unwrap().basic_account(&sender).unwrap();
        assert_eq!(producer_account, follower_account);
        assert_eq!(producer_account.unwrap().nonce, 4 * TRANSACTIONS_PER_BLOCK);
        let wire = follower.peer.trace();
        assert!(wire
            .iter()
            .any(|event| matches!(event, WireEvent::Response { headers: true, .. })));
        assert!(wire
            .iter()
            .any(|event| matches!(event, WireEvent::Response { headers: false, .. })));
        assert!(follower.peer.stats().fragments.into_iter().all(|count| count > 1));
        assert_eq!(follower.peer.bad_messages(), 0);
        let producer_prewarming = Arc::clone(&producer.prewarming);
        let follower_prewarming = Arc::clone(&follower.prewarming);
        producer.shutdown().await;
        follower.shutdown().await;
        // Drop every native handle and volatile overlay, reopen the datadir, then execute another
        // transaction through a new payload service and engine.
        let restart_overlay = OverlayManager::default();
        let restart_factory = follower_storage.open(restart_overlay.clone(), native.clone());
        assert_eq!(restart_factory.check_consistency().unwrap(), (None, None));
        let restarted = Node::launch(
            restart_factory,
            restart_overlay,
            blocks,
            TaskRuntime::deterministic(context.child("restarted")),
            native.clone(),
            seed.wrapping_add(2),
        )
        .await;
        assert_eq!(restarted.provider.best_block_number().unwrap(), 4);
        let five =
            restarted.build(four.block().sealed_header(), 4 * TRANSACTIONS_PER_BLOCK, 1).await;
        restarted.import(&five).await;
        let restarted_prewarming = Arc::clone(&restarted.prewarming);
        restarted.shutdown().await;
        let final_factory = follower_storage.open(OverlayManager::default(), native);
        assert_eq!(final_factory.check_consistency().unwrap(), (None, None));
        let persisted = BlockchainProvider::new(final_factory).unwrap();
        let sender = Address::from_str("f39fd6e51aad88f6f4ce6ab8827279cfffb92266").unwrap();
        let account = persisted.latest().unwrap().basic_account(&sender).unwrap().unwrap();
        assert_eq!(account.nonce, 5 * TRANSACTIONS_PER_BLOCK);
        assert_eq!(persisted.best_block_number().unwrap(), 5);
        assert_eq!(persisted.block_hash(5).unwrap(), Some(five.block().hash()));
        let prewarmed_transactions =
            [producer_prewarming, follower_prewarming, restarted_prewarming]
                .map(|counter| counter.load(std::sync::atomic::Ordering::Relaxed));
        assert!(
            prewarmed_transactions.into_iter().all(|count| count > 0),
            "no speculative execution on a node: {prewarmed_transactions:?}"
        );
        NodeOutcome {
            audit: context.auditor().state(),
            canonical,
            first_block: one.block().clone(),
            wire,
            persisted_head: five.block().hash(),
            sender_nonce: account.nonce,
            sender_balance: account.balance,
            prewarmed_transactions,
        }
    });
    drop(producer_storage);
    outcome
}

#[test]
fn deterministic_node_executes_downloads_persists_and_restarts() {
    // Debug EVM/provider frames and all async node components share the simulator's runner
    // thread, so reserve more stack than the test harness's default.
    std::thread::Builder::new()
        .name("node-simulation".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(run_node_campaign)
        .unwrap()
        .join()
        .unwrap();
}

fn run_node_campaign() {
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(err) => panic!("invalid RETH_DST_SEED: {err}"),
    };
    for (index, seed) in seeds.into_iter().enumerate() {
        eprintln!("node seed={seed}");
        let outcome = simulate_node(seed);
        assert_eq!(outcome, simulate_node(seed), "replay diverged for seed {seed}");
        if index == 0 {
            super::native_validation::assert_native_validation(node_chain(), outcome.first_block);
        }
    }
}
