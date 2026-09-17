//! A seeded node-core integration: real transactions, payload construction, EVM validation,
//! forkchoice, live block download, persistence, and engine restart.
//!
//! Native database operations and synchronous EVM execution are atomic simulation steps. The
//! production node launcher, RPC sockets, discovery, and parallel execution workers are outside
//! this profile. No pre-executed blocks are inserted into either engine.

use crate::{
    node_storage::{DatabaseFaults, DatabaseOperation, Factory, NodeStorage, NodeTypes},
    node_wire::{WireBlockClient, WireEvent, WirePeer},
};
use abi_fuzz::{generators::RandomGenerator, Generator as AbiGenerator};
use alloy_consensus::{BlockHeader as _, SignableTransaction, TxEip1559, TxEip2930, TxLegacy};
use alloy_dyn_abi::{DynSolType, DynSolValue};
use alloy_eips::eip2930::AccessList;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{keccak256, Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::{ExecutionData, ForkchoiceState, PayloadId, PayloadStatus};
use alloy_signer::SignerSync;
use commonware_runtime::{
    deterministic::{self, RunnableTask, SchedulingPolicy},
    Name as TaskName, Runner, Supervisor,
};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpec, ChainSpecBuilder, ChainSpecProvider, MAINNET};
use reth_db_common::init::init_genesis_with_settings;
use reth_dst::{
    ActorId, DecisionDomain, DecisionOption, DecisionPoint, DecisionPointId, DecisionSource,
    DecisionTrace, Explore, OptionId, Record, Replay, ReplayError, TraceHeader, TraceRecorder,
};
use reth_engine_primitives::{BeaconEngineMessage, NoopInvalidBlockHook};
use reth_engine_tree::{
    chain::{ChainHandler, FromOrchestrator, HandlerEvent},
    download::BasicBlockDownloader,
    engine::{EngineApiKind, EngineApiRequest, EngineApiRequestHandler, EngineHandler, FromEngine},
    persistence::{PersistenceError, PersistenceHandle},
    tree::{
        payload_processor, BasicEngineValidator, EngineApiTreeHandler, TreeConfig,
        TxPoolPrewarmCacheSnapshot, TxPoolPrewarmSource, TxPoolPrewarmTransaction,
        TxPoolPrewarmTransactions,
    },
};
use reth_eth_wire::simulation::LinkConfig;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes, EthPayloadAttributes};
use reth_ethereum_payload_builder::{EthereumBuilderConfig, EthereumPayloadBuilder};
use reth_ethereum_primitives::{Block, EthPrimitives, TransactionSigned};
use reth_evm_ethereum::EthEvmConfig;
use reth_exex_types::FinishedExExHeight;
use reth_node_ethereum::EthereumEngineValidator;
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_payload_primitives::PayloadKind;
use reth_primitives_traits::{Block as _, SealedBlock, SealedHeader, SignerRecoverable};
use reth_provider::{
    providers::BlockchainProvider, AccountReader, BlockHashReader, BlockNumReader, BlockReader,
    DatabaseProviderFactory, StateProviderBox, StateProviderFactory, StateRootProvider,
    StorageSettings,
};
use reth_prune::Pruner;
use reth_storage_overlay::OverlayManager;
use reth_tasks::{TaskHandle, TaskRuntime};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore,
    validate::{EthTransactionValidator, EthTransactionValidatorBuilder},
    BestTransactions, BestTransactionsAttributes, CoinbaseTipOrdering, EthPooledTransaction, Pool,
    PoolTransaction, TransactionOrigin, TransactionPool, TransactionPoolExt,
};
use reth_trie::HashedPostState;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use txgen_core::{
    AccountManager, AccountPoolDef, AccountRef, ArtifactManager, BuildContext, GasConfig,
    NonceTracker, SelectMode,
};

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

#[derive(Clone, Copy)]
struct NodeCampaignConfig {
    persistence_threshold: u64,
    state_masking_blocks: u64,
    multiproof_chunk_size: usize,
    inject_database_fault: bool,
    native_workers: bool,
}

impl NodeCampaignConfig {
    fn from_seed(seed: u64) -> Self {
        let persistence_threshold = 1 + seed % 4;
        let native_workers = std::env::var_os("RETH_DST_NATIVE_WORKERS").is_some();
        Self {
            persistence_threshold,
            state_masking_blocks: (seed >> 2) % persistence_threshold,
            multiproof_chunk_size: 1 + ((seed >> 5) % 16) as usize,
            // An in-process abort cannot reproduce the OS cleanup that releases RocksDB's lock
            // after a process crash. Database-fault recovery remains covered by the cooperative
            // lane, where every task holding a provider is controlled by the simulator.
            inject_database_fault: seed.is_multiple_of(4) && !native_workers,
            native_workers,
        }
    }
}

impl Node {
    async fn launch(
        factory: Factory,
        overlay: OverlayManager,
        blocks: Arc<Mutex<BTreeMap<u64, SealedBlock<Block>>>>,
        tasks: TaskRuntime,
        native: reth_tasks::Runtime,
        seed: u64,
        campaign_config: NodeCampaignConfig,
    ) -> Self {
        let provider = BlockchainProvider::new(factory.clone()).unwrap();
        let chain = provider.chain_spec();
        let evm = EthEvmConfig::new(chain.clone());
        let consensus = Arc::new(EthBeaconConsensus::new(chain.clone()));
        let config = TreeConfig::default()
            .with_cross_block_cache_size(1024 * 1024)
            .with_num_state_masking_blocks(0)
            .with_memory_block_buffer_target(0)
            .with_persistence_threshold(campaign_config.persistence_threshold)
            .with_persistence_backpressure_threshold(campaign_config.persistence_threshold + 2)
            .with_num_state_masking_blocks(campaign_config.state_masking_blocks)
            .with_multiproof_chunk_size(campaign_config.multiproof_chunk_size);
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
        let mut validator = BasicEngineValidator::new(
            provider.clone(),
            consensus.clone(),
            evm.clone(),
            EthereumEngineValidator::new(chain),
            config.clone(),
            Box::new(NoopInvalidBlockHook::default()),
            overlay.clone(),
            native.clone(),
        );
        if !campaign_config.native_workers {
            validator =
                validator.with_cooperative_sparse_trie(tasks.clone()).with_cooperative_prewarming();
        }
        let validator = validator.with_txpool_prewarming(PoolPrewarmSource(pool.clone()));
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
                capacity: 4096,
                max_chunk: 1024,
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
                    HandlerEvent::FatalError => return,
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

    async fn begin_build(
        &self,
        parent: &SealedHeader,
        parent_nonces: AccountNonces,
        branch: u8,
        transaction_count: usize,
    ) -> PayloadBuild {
        // The harness supplies pool head maintenance while the production maintenance actor
        // remains outside this profile. The prewarmer uses the same live best-tx iterator.
        let mut info = self.pool.block_info();
        info.last_seen_block_hash = parent.hash();
        info.last_seen_block_number = parent.number;
        info.block_gas_limit = parent.gas_limit;
        self.pool.set_block_info(info);
        let (transactions, next_nonces) =
            materialize_block_transactions(parent.hash(), parent_nonces, branch, transaction_count);
        let mut hashes = Vec::new();
        let mut expected_parent_nonces = BTreeMap::new();
        for (transaction, sender, parent_nonce) in transactions {
            expected_parent_nonces.entry(sender).or_insert(parent_nonce);
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
        loop {
            if let Some(snapshot) = (self.txpool_snapshot)(parent.hash()) {
                let all_senders_warm = expected_parent_nonces.iter().all(|(sender, nonce)| {
                    snapshot.account(sender).is_some_and(|account| {
                        account.is_some_and(|account| account.nonce == *nonce)
                    })
                });
                if all_senders_warm {
                    break
                }
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
        PayloadBuild {
            payload_id: response.payload_id.expect("FCU started payload job"),
            transaction_hashes: hashes,
            parent: parent.hash(),
            branch,
            transaction_count,
            next_nonces,
        }
    }

    async fn poll_build(&self, build: &PayloadBuild) -> Option<EthBuiltPayload> {
        let payload = self
            .payload_builder
            .resolve_kind(build.payload_id, PayloadKind::WaitForPending)
            .await?
            .unwrap();
        self.pool.remove_transactions(build.transaction_hashes.clone());
        assert_eq!(payload.block().body().transactions.len(), build.transaction_count);
        assert!(payload.block().gas_used() >= 21_000 * build.transaction_count as u64);
        Some(payload)
    }

    async fn build(
        &self,
        parent: &SealedHeader,
        parent_nonces: AccountNonces,
        branch: u8,
        transaction_count: usize,
    ) -> (EthBuiltPayload, AccountNonces) {
        let build = self.begin_build(parent, parent_nonces, branch, transaction_count).await;
        loop {
            if let Some(payload) = self.poll_build(&build).await {
                return (payload, build.next_nonces)
            }
            self.tasks.sleep(Duration::from_millis(1)).await;
        }
    }

    async fn new_payload(&self, payload: ExecutionData) -> PayloadStatus {
        let (tx, rx) = oneshot::channel();
        self.input.send(BeaconEngineMessage::NewPayload { payload, tx }).unwrap();
        rx.await.unwrap().unwrap()
    }

    async fn forkchoice(&self, head: B256) -> PayloadStatus {
        self.try_forkchoice(head).await.expect("engine stopped before forkchoice response")
    }

    async fn try_forkchoice(&self, head: B256) -> Option<PayloadStatus> {
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
            .ok()?;
        Some(rx.await.ok()?.ok()?.await.ok()?.payload_status)
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

    fn sparse_trie_frontier(&self) -> Option<(B256, B256)> {
        let trie = self.overlay.take_sparse_trie()?;
        let frontier = (trie.anchor_hash(), trie.state_root());
        self.overlay.store_sparse_trie(trie);
        Some(frontier)
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

    async fn crash(self) {
        self.peer.disconnect();
        self.router.abort();
        self.payload_service.abort();
        self.engine.abort();
        self.persistence.abort();
        let _ = self.router.await;
        let _ = self.payload_service.await;
        let _ = self.engine.await;
        let _ = self.persistence.await;
        self.peer.shutdown().await;
    }
}

type Provider = BlockchainProvider<NodeTypes>;
type TxPool = Pool<
    EthTransactionValidator<Provider, EthPooledTransaction, EthEvmConfig>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    InMemoryBlobStore,
>;

type AccountNonces = [u64; WORKLOAD_ACCOUNT_COUNT];

const WORKLOAD_ACCOUNT_COUNT: usize = 20;
const STORAGE_CONTRACT_COUNT: u8 = 4;
const WORKLOAD_MNEMONIC: &str = "test test test test test test test test test test test junk";
const STORAGE_RUNTIME_CODE: &[u8] = &[
    0x60, 0x20, 0x35, 0x60, 0x00, 0x35, 0x55, 0x60, 0x60, 0x35, 0x60, 0x40, 0x35, 0x55, 0x60, 0xa0,
    0x35, 0x60, 0x80, 0x35, 0x55, 0x60, 0xe0, 0x35, 0x60, 0xc0, 0x35, 0x55, 0x00,
];
const STORAGE_INIT_CODE: &[u8] = &[
    0x67, 0x60, 0x20, 0x35, 0x60, 0x00, 0x35, 0x55, 0x00, 0x60, 0x00, 0x52, 0x60, 0x08, 0x60, 0x18,
    0xf3,
];

fn storage_contract(index: u8) -> Address {
    debug_assert!(index < STORAGE_CONTRACT_COUNT);
    Address::repeat_byte(0xcc + index)
}

fn workload_accounts() -> &'static AccountManager {
    static ACCOUNTS: OnceLock<AccountManager> = OnceLock::new();
    ACCOUNTS.get_or_init(|| {
        AccountManager::from_spec(&HashMap::from([(
            "users".to_owned(),
            AccountPoolDef {
                mnemonic: WORKLOAD_MNEMONIC.to_owned(),
                index: None,
                range: Some([0, WORKLOAD_ACCOUNT_COUNT as u32]),
            },
        )]))
        .unwrap()
    })
}

fn workload_addresses() -> [Address; WORKLOAD_ACCOUNT_COUNT] {
    std::array::from_fn(|index| workload_accounts().get_by_index("users", index).unwrap().address())
}

fn materialize_block_transactions(
    parent: B256,
    parent_nonces: AccountNonces,
    branch: u8,
    count: usize,
) -> (Vec<(TransactionSigned, Address, u64)>, AccountNonces) {
    let mut seed_material = Vec::with_capacity(33);
    seed_material.extend_from_slice(parent.as_slice());
    seed_material.push(branch);
    let seed: [u8; 32] = keccak256(seed_material).into();
    let mut rng = StdRng::from_seed(seed);
    let accounts = workload_accounts();
    let artifacts = ArtifactManager::empty();
    let gas =
        GasConfig { max_fee_per_gas: 1_000_000_000_000, max_priority_fee_per_gas: 20_000_000_000 };
    let addresses = workload_addresses();
    let mut nonces = NonceTracker::new();
    for (index, address) in addresses.iter().enumerate() {
        nonces.reset(address.0 .0, parent_nonces[index]);
    }
    let mut context = BuildContext::new(1, &gas, accounts, &artifacts, &mut nonces, &mut rng);
    let mut abi = RandomGenerator {
        uint: abi_fuzz::generators::UintDistribution::Mixed(
            abi_fuzz::generators::UintGenerator::new(256, []),
        ),
        ..Default::default()
    };
    let mut next_nonces = parent_nonces;
    let mut transactions = Vec::with_capacity(count);

    for _ in 0..count {
        let sender_index = context.rng.next_u64() as usize % WORKLOAD_ACCOUNT_COUNT;
        let selected = context
            .select_signer(&AccountRef {
                pool: "users".to_owned(),
                select: SelectMode::Index(sender_index),
            })
            .unwrap();
        let sender = selected.address;
        let parent_nonce = parent_nonces[sender_index];
        let nonce = context.next_nonce(sender.0 .0);
        let action = context.rng.next_u64() % 4;
        let (to, input, value, gas_limit) = if action < 2 {
            let mut input = Vec::with_capacity(256);
            for _ in 0..4 {
                let key = match abi.generate(&DynSolType::Uint(256), context.rng) {
                    DynSolValue::Uint(value, _) => value,
                    _ => unreachable!(),
                };
                let value = match abi.generate(&DynSolType::Uint(256), context.rng) {
                    DynSolValue::Uint(value, _) => value,
                    _ => unreachable!(),
                };
                input.extend_from_slice(&key.to_be_bytes::<32>());
                input.extend_from_slice(&value.to_be_bytes::<32>());
            }
            let contract = storage_contract(context.rng.next_u64() as u8 % STORAGE_CONTRACT_COUNT);
            (Some(contract), Bytes::from(input), U256::ZERO, 200_000)
        } else if action == 2 {
            let recipient = addresses[context.rng.next_u64() as usize % WORKLOAD_ACCOUNT_COUNT];
            (Some(recipient), Bytes::new(), U256::from(context.rng.next_u64() % 1_000), 21_000)
        } else {
            (None, Bytes::from_static(STORAGE_INIT_CODE), U256::ZERO, 200_000)
        };
        let to = to.map_or(TxKind::Create, TxKind::Call);
        let signer = accounts.get_by_index(&selected.pool, selected.index).unwrap();
        let transaction = match context.rng.next_u64() % 3 {
            0 => {
                let tx = TxLegacy {
                    chain_id: Some(1),
                    nonce,
                    gas_price: gas.max_fee_per_gas,
                    gas_limit,
                    to,
                    value,
                    input,
                };
                let signature = signer.sign_hash_sync(&tx.signature_hash()).unwrap();
                TransactionSigned::from(tx.into_signed(signature))
            }
            1 => {
                let tx = TxEip2930 {
                    chain_id: 1,
                    nonce,
                    gas_price: gas.max_fee_per_gas,
                    gas_limit,
                    to,
                    value,
                    access_list: AccessList::default(),
                    input,
                };
                let signature = signer.sign_hash_sync(&tx.signature_hash()).unwrap();
                TransactionSigned::from(tx.into_signed(signature))
            }
            _ => {
                let tx = TxEip1559 {
                    chain_id: 1,
                    nonce,
                    gas_limit,
                    max_fee_per_gas: gas.max_fee_per_gas,
                    max_priority_fee_per_gas: gas.max_priority_fee_per_gas,
                    to,
                    value,
                    access_list: AccessList::default(),
                    input,
                };
                let signature = signer.sign_hash_sync(&tx.signature_hash()).unwrap();
                TransactionSigned::from(tx.into_signed(signature))
            }
        };
        context.take_nonce_reservations();
        transactions.push((transaction, sender, parent_nonce));
        next_nonces[sender_index] += 1;
    }

    (transactions, next_nonces)
}

const MIN_TRANSACTIONS_PER_BLOCK: usize = payload_processor::SMALL_BLOCK_TX_THRESHOLD;
const MAX_TRANSACTIONS_PER_BLOCK: usize = 64;
const CAMPAIGN_SCHEMA_VERSION: u64 = 12;
const MAX_DATABASE_FAULTS_PER_CASE: u64 = 3;

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
    accounts: Vec<(Address, u64, U256)>,
    prewarmed_transactions: [usize; 3],
    trie_frontier_reuses: u64,
}

fn node_chain() -> Arc<ChainSpec> {
    let mut genesis: Genesis = serde_json::from_str(include_str!(
        "../../../crates/e2e-test-utils/src/testsuite/assets/genesis.json"
    ))
    .unwrap();
    for index in 0..STORAGE_CONTRACT_COUNT {
        genesis.alloc.insert(
            storage_contract(index),
            GenesisAccount {
                code: Some(Bytes::copy_from_slice(STORAGE_RUNTIME_CODE)),
                ..Default::default()
            },
        );
    }
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CampaignAction {
    BeginPayloadBuild { parent: B256, branch: u8 },
    PollPayloadBuild,
    ImportPayload { block: B256 },
    CorruptPayload { block: B256, field: CorruptField },
    SetHead { head: B256 },
    BeginFollowerSync { head: B256 },
    PollFollowerSync,
    PartitionFollower,
    HealFollower,
    AdvanceTime,
    CrashRestartFollower,
    GracefulRestartFollower,
    CorruptFollowerResponse,
    ReplayPayload { block: B256 },
    EnableDatabaseFault,
    BeginStateRead { block: B256 },
    CompleteStateRead,
}

impl CampaignAction {
    fn encoded(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(34);
        match self {
            Self::BeginPayloadBuild { parent, branch } => {
                encoded.push(0);
                encoded.extend_from_slice(parent.as_slice());
                encoded.push(*branch);
            }
            Self::PollPayloadBuild => encoded.push(13),
            Self::ImportPayload { block } => {
                encoded.push(1);
                encoded.extend_from_slice(block.as_slice());
            }
            Self::CorruptPayload { block, field } => {
                encoded.push(2);
                encoded.extend_from_slice(block.as_slice());
                encoded.push(*field as u8);
            }
            Self::SetHead { head } => {
                encoded.push(3);
                encoded.extend_from_slice(head.as_slice());
            }
            Self::BeginFollowerSync { head } => {
                encoded.push(4);
                encoded.extend_from_slice(head.as_slice());
            }
            Self::PollFollowerSync => encoded.push(5),
            Self::PartitionFollower => encoded.push(6),
            Self::HealFollower => encoded.push(7),
            Self::AdvanceTime => encoded.push(8),
            Self::CrashRestartFollower => encoded.push(9),
            Self::GracefulRestartFollower => encoded.push(16),
            Self::CorruptFollowerResponse => encoded.push(10),
            Self::ReplayPayload { block } => {
                encoded.push(11);
                encoded.extend_from_slice(block.as_slice());
            }
            Self::EnableDatabaseFault => encoded.push(12),
            Self::BeginStateRead { block } => {
                encoded.push(14);
                encoded.extend_from_slice(block.as_slice());
            }
            Self::CompleteStateRead => encoded.push(15),
        }
        encoded
    }

    fn option(&self) -> DecisionOption {
        let payload = self.encoded();
        let digest = keccak256(&payload);
        let mut id = [0; 8];
        id.copy_from_slice(&digest[..8]);
        DecisionOption::new(OptionId(u64::from_be_bytes(id)), payload, format!("{self:?}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum CorruptField {
    ReceiptsRoot,
    StateRoot,
}

#[derive(Clone)]
struct ModeledBlock {
    header: SealedHeader,
    payload: Option<EthBuiltPayload>,
    parent: Option<B256>,
    next_nonces: AccountNonces,
    children: u8,
    replayed: bool,
}

struct PendingPayload {
    payload: EthBuiltPayload,
    parent: B256,
    next_nonces: AccountNonces,
    corrupt_receipts_checked: bool,
    corrupt_state_checked: bool,
}

struct PayloadBuild {
    payload_id: PayloadId,
    transaction_hashes: Vec<B256>,
    parent: B256,
    branch: u8,
    transaction_count: usize,
    next_nonces: AccountNonces,
}

struct StateRead {
    provider: StateProviderBox,
    block: B256,
    state_root: B256,
    nonces: AccountNonces,
}

struct CampaignModel {
    actor: ActorId,
    occurrences: BTreeMap<DecisionDomain, u64>,
    blocks: BTreeMap<B256, ModeledBlock>,
    canonical_head: B256,
    follower_head: B256,
    follower_sync_target: Option<B256>,
    follower_partitioned: bool,
    follower_restarts: u32,
    pending: BTreeMap<B256, PendingPayload>,
    payload_build: Option<PayloadBuild>,
    state_read: Option<StateRead>,
    follower_wire: Vec<WireEvent>,
    maximum_blocks: usize,
    database_faults: Arc<CampaignDatabaseFaults>,
    trie_frontier_reuses: u64,
    allow_in_process_crash: bool,
}

impl CampaignModel {
    fn new(
        genesis: SealedHeader,
        maximum_blocks: usize,
        database_faults: Arc<CampaignDatabaseFaults>,
        allow_in_process_crash: bool,
    ) -> Self {
        let hash = genesis.hash();
        Self {
            actor: ActorId::new("node-campaign").unwrap(),
            occurrences: BTreeMap::new(),
            blocks: BTreeMap::from([(
                hash,
                ModeledBlock {
                    header: genesis,
                    payload: None,
                    parent: None,
                    next_nonces: [0; WORKLOAD_ACCOUNT_COUNT],
                    children: 0,
                    replayed: true,
                },
            )]),
            canonical_head: hash,
            follower_head: hash,
            follower_sync_target: None,
            follower_partitioned: false,
            follower_restarts: 0,
            pending: BTreeMap::new(),
            payload_build: None,
            state_read: None,
            follower_wire: Vec::new(),
            maximum_blocks,
            database_faults,
            trie_frontier_reuses: 0,
            allow_in_process_crash,
        }
    }

    fn legal_actions(&self) -> Vec<CampaignAction> {
        if self.database_faults.needs_recovery() {
            return vec![CampaignAction::CrashRestartFollower]
        }
        if self.blocks[&self.canonical_head].header.number >= 4 &&
            self.database_faults.injected() == 0 &&
            self.database_faults.can_arm()
        {
            return vec![CampaignAction::EnableDatabaseFault]
        }
        if self.state_read.is_some() {
            // An unwind commit waits synchronously for old MDBX readers. Finish the modeled read
            // before another workload action can request an unwind; scheduler tasks can still run
            // between the begin and completion decisions.
            return vec![CampaignAction::CompleteStateRead]
        }
        if self.follower_sync_target.is_some() {
            let mut actions = vec![
                CampaignAction::PollFollowerSync,
                CampaignAction::AdvanceTime,
                CampaignAction::CorruptFollowerResponse,
            ];
            if self.allow_in_process_crash && !self.database_faults.is_armed() {
                actions.push(CampaignAction::CrashRestartFollower);
            }
            if self.database_faults.can_arm() {
                actions.push(CampaignAction::EnableDatabaseFault);
            }
            actions.push(if self.follower_partitioned {
                CampaignAction::HealFollower
            } else {
                CampaignAction::PartitionFollower
            });
            actions.sort_unstable();
            return actions
        }

        if self.payload_build.is_some() {
            let mut actions = vec![CampaignAction::PollPayloadBuild, CampaignAction::AdvanceTime];
            actions.extend(
                self.blocks
                    .keys()
                    .filter(|hash| **hash != self.canonical_head)
                    .map(|head| CampaignAction::SetHead { head: *head }),
            );
            actions.sort_unstable();
            actions.dedup();
            return actions
        }

        if !self.pending.is_empty() {
            let mut actions = Vec::new();
            if self.database_faults.can_arm() {
                actions.push(CampaignAction::EnableDatabaseFault);
            }
            for (hash, pending) in &self.pending {
                actions.push(CampaignAction::ImportPayload { block: *hash });
                if !pending.corrupt_receipts_checked {
                    actions.push(CampaignAction::CorruptPayload {
                        block: *hash,
                        field: CorruptField::ReceiptsRoot,
                    });
                }
                if !pending.corrupt_state_checked {
                    actions.push(CampaignAction::CorruptPayload {
                        block: *hash,
                        field: CorruptField::StateRoot,
                    });
                }
            }
            let parent = self.pending.values().next().unwrap().parent;
            let parent_block = self.blocks.get(&parent).unwrap();
            if self.pending.len() < 3 && self.canonical_head == parent && parent_block.children < 4
            {
                actions.push(CampaignAction::BeginPayloadBuild {
                    parent,
                    branch: parent_block.children,
                });
            }
            actions.sort_unstable();
            return actions
        }

        let canonical = self.blocks.get(&self.canonical_head).unwrap();
        if canonical.header.number < 4 {
            return vec![CampaignAction::BeginPayloadBuild {
                parent: self.canonical_head,
                branch: canonical.children,
            }]
        }

        let mut actions = Vec::new();
        if self.blocks.len() < self.maximum_blocks && canonical.children < 4 {
            actions.push(CampaignAction::BeginPayloadBuild {
                parent: self.canonical_head,
                branch: canonical.children,
            });
        }
        actions.extend(
            self.blocks
                .keys()
                .filter(|hash| **hash != self.canonical_head)
                .map(|head| CampaignAction::SetHead { head: *head }),
        );
        actions.extend(self.blocks.iter().filter_map(|(hash, block)| {
            (block.payload.is_some() && !block.replayed)
                .then_some(CampaignAction::ReplayPayload { block: *hash })
        }));
        if self.follower_head != self.canonical_head {
            actions.push(CampaignAction::BeginFollowerSync { head: self.canonical_head });
        }
        actions.push(CampaignAction::AdvanceTime);
        if self.allow_in_process_crash && !self.database_faults.is_armed() {
            actions.push(CampaignAction::CrashRestartFollower);
        }
        if self.database_faults.can_arm() {
            actions.push(CampaignAction::EnableDatabaseFault);
        }
        actions.push(CampaignAction::BeginStateRead { block: self.canonical_head });
        actions.sort_unstable();
        actions.dedup();
        actions
    }

    fn choose(
        &mut self,
        source: &mut dyn DecisionSource,
        tasks: &TaskRuntime,
        actions: Vec<CampaignAction>,
    ) -> CampaignAction {
        assert!(!actions.is_empty(), "campaign has no legal action");
        let domain = DecisionDomain::Workload;
        let occurrence = self.next_occurrence(domain);
        let options = actions.iter().map(CampaignAction::option).collect();
        let point = DecisionPoint::new(
            DecisionPointId { domain, actor: self.actor.clone(), generation: 0, occurrence },
            virtual_micros(tasks),
            self.state_digest(),
            options,
        )
        .unwrap();
        let selected = source.choose(&point).unwrap();
        actions
            .into_iter()
            .find(|action| action.option().id() == selected)
            .expect("decision source returned an option absent from the point")
    }

    fn choose_transaction_count(
        &mut self,
        source: &mut dyn DecisionSource,
        tasks: &TaskRuntime,
    ) -> usize {
        let domain = DecisionDomain::Workload;
        let occurrence = self.next_occurrence(domain);
        let choices = (MIN_TRANSACTIONS_PER_BLOCK..=MAX_TRANSACTIONS_PER_BLOCK)
            .map(|count| {
                (
                    count,
                    DecisionOption::new(
                        OptionId(count as u64),
                        vec![count as u8],
                        format!("build block with {count} transactions"),
                    ),
                )
            })
            .collect::<Vec<_>>();
        let point = DecisionPoint::new(
            DecisionPointId { domain, actor: self.actor.clone(), generation: 0, occurrence },
            virtual_micros(tasks),
            self.state_digest(),
            choices.iter().map(|(_, option)| option.clone()).collect(),
        )
        .unwrap();
        let selected = source.choose(&point).unwrap();
        choices
            .into_iter()
            .find_map(|(count, option)| (option.id() == selected).then_some(count))
            .expect("decision source returned a transaction count absent from the point")
    }

    fn choose_time_advance(
        &mut self,
        source: &mut dyn DecisionSource,
        tasks: &TaskRuntime,
    ) -> Duration {
        let domain = DecisionDomain::Timer;
        let occurrence = self.next_occurrence(domain);
        let choices = (1u64..=25)
            .map(|millis| {
                (
                    millis,
                    DecisionOption::new(
                        OptionId(millis),
                        millis.to_be_bytes().to_vec(),
                        format!("advance virtual time by {millis}ms"),
                    ),
                )
            })
            .collect::<Vec<_>>();
        let point = DecisionPoint::new(
            DecisionPointId { domain, actor: self.actor.clone(), generation: 0, occurrence },
            virtual_micros(tasks),
            self.state_digest(),
            choices.iter().map(|(_, option)| option.clone()).collect(),
        )
        .unwrap();
        let selected = source.choose(&point).unwrap();
        let millis = choices
            .into_iter()
            .find_map(|(millis, option)| (option.id() == selected).then_some(millis))
            .expect("decision source returned a time advance absent from the point");
        Duration::from_millis(millis)
    }

    fn record_event(
        &mut self,
        source: &mut dyn DecisionSource,
        tasks: &TaskRuntime,
        domain: DecisionDomain,
        payload: Vec<u8>,
        summary: &str,
    ) {
        let occurrence = self.next_occurrence(domain);
        let option = DecisionOption::new(OptionId(0), payload, summary);
        let point = DecisionPoint::new(
            DecisionPointId {
                domain,
                actor: self.actor.clone(),
                generation: u32::from(matches!(domain, DecisionDomain::NodeLifecycle)),
                occurrence,
            },
            virtual_micros(tasks),
            self.state_digest(),
            vec![option],
        )
        .unwrap();
        assert_eq!(source.choose(&point).unwrap(), OptionId(0));
    }

    fn next_occurrence(&mut self, domain: DecisionDomain) -> u64 {
        let occurrence = self.occurrences.entry(domain).or_default();
        let current = *occurrence;
        *occurrence += 1;
        current
    }

    fn state_digest(&self) -> B256 {
        let mut state = Vec::new();
        state.extend_from_slice(self.canonical_head.as_slice());
        state.extend_from_slice(self.follower_head.as_slice());
        if let Some(target) = self.follower_sync_target {
            state.push(1);
            state.extend_from_slice(target.as_slice());
        } else {
            state.push(0);
        }
        state.push(u8::from(self.follower_partitioned));
        state.extend_from_slice(&self.follower_restarts.to_be_bytes());
        state.extend_from_slice(&(self.blocks.len() as u64).to_be_bytes());
        for (hash, block) in &self.blocks {
            state.extend_from_slice(hash.as_slice());
            state.extend_from_slice(&block.header.number.to_be_bytes());
            for nonce in block.next_nonces {
                state.extend_from_slice(&nonce.to_be_bytes());
            }
            state.push(block.children);
            state.push(u8::from(block.replayed));
            if let Some(parent) = block.parent {
                state.push(1);
                state.extend_from_slice(parent.as_slice());
            } else {
                state.push(0);
            }
        }
        state.extend_from_slice(&(self.pending.len() as u64).to_be_bytes());
        if let Some(build) = &self.payload_build {
            state.push(1);
            state.extend_from_slice(build.payload_id.0.as_slice());
            state.extend_from_slice(build.parent.as_slice());
            state.push(build.branch);
            state.extend_from_slice(&(build.transaction_count as u64).to_be_bytes());
            for nonce in build.next_nonces {
                state.extend_from_slice(&nonce.to_be_bytes());
            }
        } else {
            state.push(0);
        }
        if let Some(read) = &self.state_read {
            state.push(1);
            state.extend_from_slice(read.block.as_slice());
            state.extend_from_slice(read.state_root.as_slice());
            for nonce in read.nonces {
                state.extend_from_slice(&nonce.to_be_bytes());
            }
        } else {
            state.push(0);
        }
        state.push(u8::from(self.database_faults.is_armed()));
        state.extend_from_slice(&self.database_faults.injected().to_be_bytes());
        state.push(u8::from(self.database_faults.needs_recovery()));
        for (hash, pending) in &self.pending {
            state.extend_from_slice(hash.as_slice());
            state.extend_from_slice(pending.payload.block().hash().as_slice());
            state.extend_from_slice(pending.parent.as_slice());
            for nonce in pending.next_nonces {
                state.extend_from_slice(&nonce.to_be_bytes());
            }
            state.push(u8::from(pending.corrupt_receipts_checked));
            state.push(u8::from(pending.corrupt_state_checked));
        }
        keccak256(state)
    }

    fn deepest_head(&self) -> B256 {
        self.blocks
            .iter()
            .max_by_key(|(hash, block)| (block.header.number, *hash))
            .map(|(hash, _)| *hash)
            .unwrap()
    }

    fn canonical_hashes(&self, head: B256) -> Vec<B256> {
        let mut hashes = Vec::new();
        let mut cursor = Some(head);
        while let Some(hash) = cursor {
            let block = self.blocks.get(&hash).expect("model parent is missing");
            hashes.push(hash);
            cursor = block.parent;
        }
        hashes.reverse();
        hashes
    }

    fn is_strict_ancestor(&self, ancestor: B256, descendant: B256) -> bool {
        let mut cursor = self.blocks.get(&descendant).and_then(|block| block.parent);
        while let Some(hash) = cursor {
            if hash == ancestor {
                return true
            }
            cursor = self.blocks.get(&hash).and_then(|block| block.parent);
        }
        false
    }

    fn first_block(&self) -> SealedBlock<Block> {
        self.blocks
            .values()
            .find(|block| block.header.number == 1)
            .and_then(|block| block.payload.as_ref())
            .expect("bootstrap did not build block one")
            .block()
            .clone()
    }
}

enum CampaignDecisions {
    Explore(Record<Explore>),
    Replay(Replay),
}

#[derive(Clone)]
struct SharedDecisions(Arc<Mutex<BoundedDecisions>>);

struct BoundedDecisions {
    source: CampaignDecisions,
    selected: usize,
    limit: usize,
}

#[derive(Debug)]
struct DecisionLimitExceeded {
    selected: usize,
}

impl SharedDecisions {
    fn new(input: CampaignInput, seed: u64, limit: usize) -> Self {
        let source = match input {
            CampaignInput::Explore(trace) => {
                CampaignDecisions::Explore(Record::with_recorder(Explore::new(seed), trace))
            }
            CampaignInput::Replay(trace) => CampaignDecisions::Replay(Replay::new(trace).unwrap()),
        };
        Self(Arc::new(Mutex::new(BoundedDecisions { source, selected: 0, limit })))
    }

    fn finish(self) {
        Arc::try_unwrap(self.0)
            .unwrap_or_else(|_| panic!("decision controller still has users after simulator stop"))
            .into_inner()
            .unwrap()
            .source
            .finish();
    }
}

struct CampaignDatabaseFaults {
    decisions: SharedDecisions,
    state: Mutex<DatabaseFaultState>,
    actor: ActorId,
    virtual_time: Arc<AtomicU64>,
    enabled: bool,
}

struct DatabaseFaultState {
    armed: bool,
    suppressed: bool,
    class: DatabaseFaultClass,
    remaining: u64,
    occurrence: u64,
    injected: u64,
    recovered: u64,
}

impl Default for DatabaseFaultState {
    fn default() -> Self {
        Self {
            armed: false,
            suppressed: false,
            class: DatabaseFaultClass::Any,
            remaining: 0,
            occurrence: 0,
            injected: 0,
            recovered: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum DatabaseFaultClass {
    Any,
    Transaction,
    Read,
    Cursor,
    Write,
    Commit,
}

impl DatabaseFaultClass {
    const fn id(self) -> u64 {
        match self {
            Self::Any => 0,
            Self::Transaction => 1,
            Self::Read => 2,
            Self::Cursor => 3,
            Self::Write => 4,
            Self::Commit => 5,
        }
    }

    fn accepts(self, operation: DatabaseOperation) -> bool {
        match self {
            Self::Any => true,
            Self::Transaction => operation.kind.contains("transaction"),
            Self::Read => matches!(operation.kind, "get" | "get-encoded" | "entries"),
            Self::Cursor => operation.kind.contains("cursor"),
            Self::Write => matches!(operation.kind, "put" | "append" | "delete" | "clear"),
            Self::Commit => operation.kind == "commit",
        }
    }
}

impl CampaignDatabaseFaults {
    fn new(decisions: SharedDecisions, virtual_time: Arc<AtomicU64>, enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            decisions,
            state: Mutex::new(DatabaseFaultState::default()),
            actor: ActorId::new("follower-database").unwrap(),
            virtual_time,
            enabled,
        })
    }

    fn adapter(self: &Arc<Self>) -> DatabaseFaults {
        let this = Arc::clone(self);
        DatabaseFaults::new(move |operation| this.decide(operation))
    }

    fn arm(&self) {
        let mut state = self.state.lock().unwrap();
        assert!(
            self.enabled &&
                !state.armed &&
                state.injected == state.recovered &&
                state.injected < MAX_DATABASE_FAULTS_PER_CASE
        );
        let plans = [
            (DatabaseFaultClass::Any, 16u64),
            (DatabaseFaultClass::Transaction, 2),
            (DatabaseFaultClass::Read, 0),
            (DatabaseFaultClass::Read, 4),
            (DatabaseFaultClass::Cursor, 0),
            (DatabaseFaultClass::Write, 0),
            (DatabaseFaultClass::Commit, 0),
        ];
        let choices = plans
            .iter()
            .map(|(class, skip)| {
                let payload = [class.id().to_be_bytes(), skip.to_be_bytes()].concat();
                DecisionOption::new(
                    OptionId(2 + class.id() * 64 + skip),
                    payload,
                    format!("fail {class:?} operation after {skip} matching operations"),
                )
            })
            .collect::<Vec<_>>();
        let point = DecisionPoint::new(
            DecisionPointId {
                domain: DecisionDomain::Storage,
                actor: self.actor.clone(),
                generation: 0,
                occurrence: state.occurrence,
            },
            self.virtual_time.load(Ordering::Acquire),
            keccak256([state.injected as u8]),
            choices,
        )
        .unwrap();
        state.occurrence += 1;
        let selected = self.decisions.clone().choose(&point).unwrap();
        let (class, remaining) = plans
            .into_iter()
            .find(|(class, skip)| OptionId(2 + class.id() * 64 + skip) == selected)
            .expect("database fault source selected an unknown plan");
        state.class = class;
        state.remaining = remaining;
        state.armed = true;
    }

    fn disarm(&self) {
        self.state.lock().unwrap().armed = false;
    }

    fn suppress(&self) {
        let mut state = self.state.lock().unwrap();
        assert!(!state.suppressed);
        state.suppressed = true;
    }

    fn resume(&self) {
        let mut state = self.state.lock().unwrap();
        assert!(state.suppressed);
        state.suppressed = false;
    }

    fn is_armed(&self) -> bool {
        self.state.lock().unwrap().armed
    }

    fn can_arm(&self) -> bool {
        let state = self.state.lock().unwrap();
        self.enabled &&
            !state.armed &&
            state.injected == state.recovered &&
            state.injected < MAX_DATABASE_FAULTS_PER_CASE
    }

    fn injected(&self) -> u64 {
        self.state.lock().unwrap().injected
    }

    fn needs_recovery(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.injected > state.recovered
    }

    fn mark_recovered(&self) {
        let mut state = self.state.lock().unwrap();
        assert_eq!(state.recovered + 1, state.injected);
        state.recovered = state.injected;
    }

    fn decide(&self, operation: DatabaseOperation) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.armed || state.suppressed {
            return false
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(operation.kind.as_bytes());
        if let Some(table) = operation.table {
            payload.push(0);
            payload.extend_from_slice(table.as_bytes());
        }
        let operation_summary = operation.table.map_or_else(
            || operation.kind.to_string(),
            |table| format!("{} {table}", operation.kind),
        );
        payload.extend_from_slice(&state.class.id().to_be_bytes());
        payload.extend_from_slice(&state.remaining.to_be_bytes());
        let eligible = state.class.accepts(operation);
        let fail = eligible && state.remaining == 0;
        let option = if fail {
            DecisionOption::new(
                OptionId(1),
                payload.clone(),
                format!("return database I/O error from {operation_summary}"),
            )
        } else {
            DecisionOption::new(
                OptionId(0),
                payload.clone(),
                format!("complete database operation {operation_summary}"),
            )
        };
        let point = DecisionPoint::new(
            DecisionPointId {
                domain: DecisionDomain::Storage,
                actor: self.actor.clone(),
                generation: 0,
                occurrence: state.occurrence,
            },
            self.virtual_time.load(Ordering::Acquire),
            keccak256(&payload),
            vec![option],
        )
        .unwrap();
        state.occurrence += 1;
        assert_eq!(
            self.decisions.clone().choose(&point).unwrap(),
            if fail { OptionId(1) } else { OptionId(0) }
        );
        if fail {
            state.armed = false;
            state.injected += 1;
        } else if eligible {
            state.remaining -= 1;
        }
        fail
    }
}

impl DecisionSource for SharedDecisions {
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError> {
        let mut decisions = self.0.lock().unwrap();
        if decisions.selected == decisions.limit {
            let selected = decisions.selected;
            drop(decisions);
            std::panic::panic_any(DecisionLimitExceeded { selected });
        }
        decisions.selected += 1;
        decisions.source.choose(point)
    }
}

struct SemanticScheduler {
    decisions: SharedDecisions,
    actor: ActorId,
    occurrence: u64,
    virtual_time: Arc<AtomicU64>,
}

impl SemanticScheduler {
    fn new(decisions: SharedDecisions, virtual_time: Arc<AtomicU64>) -> Self {
        Self {
            decisions,
            actor: ActorId::new("commonware-executor").unwrap(),
            occurrence: 0,
            virtual_time,
        }
    }
}

impl SchedulingPolicy for SemanticScheduler {
    fn order(&mut self, virtual_time: std::time::SystemTime, ready: &mut [RunnableTask]) {
        let virtual_time = virtual_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros()
            .try_into()
            .unwrap();
        self.virtual_time.store(virtual_time, Ordering::Release);
        let mut prefix = Vec::new();
        for index in 0..ready.len().saturating_sub(1) {
            let options = ready[index..]
                .iter()
                .map(|task| {
                    schedule_option(
                        task.name(),
                        task.generation(),
                        task.activation(),
                        task.is_root(),
                    )
                })
                .collect::<Vec<_>>();
            let point = DecisionPoint::new(
                DecisionPointId {
                    domain: DecisionDomain::Schedule,
                    actor: self.actor.clone(),
                    generation: 0,
                    occurrence: self.occurrence,
                },
                virtual_time,
                keccak256(&prefix),
                options,
            )
            .unwrap();
            self.occurrence += 1;
            let selected = self.decisions.choose(&point).unwrap();
            let offset = ready[index..]
                .iter()
                .position(|task| {
                    schedule_option(
                        task.name(),
                        task.generation(),
                        task.activation(),
                        task.is_root(),
                    )
                    .id() ==
                        selected
                })
                .expect("scheduler selected a task outside the ready set");
            ready.swap(index, index + offset);
            prefix.extend_from_slice(&selected.0.to_be_bytes());
        }
    }
}

fn schedule_option(
    name: &TaskName,
    generation: u64,
    activation: u64,
    root: bool,
) -> DecisionOption {
    let mut payload = vec![u8::from(root)];
    payload.extend_from_slice(&generation.to_be_bytes());
    payload.extend_from_slice(&activation.to_be_bytes());
    payload.extend_from_slice(&(name.label.len() as u64).to_be_bytes());
    payload.extend_from_slice(name.label.as_bytes());
    for (key, value) in &name.attributes {
        payload.extend_from_slice(&(key.len() as u64).to_be_bytes());
        payload.extend_from_slice(key.as_bytes());
        payload.extend_from_slice(&(value.len() as u64).to_be_bytes());
        payload.extend_from_slice(value.as_bytes());
    }
    let digest = keccak256(&payload);
    let id = OptionId(u64::from_be_bytes(digest[..8].try_into().unwrap()));
    let summary = if root {
        "run simulator root".to_string()
    } else {
        format!(
            "run {} {:?} generation {generation} activation {activation}",
            name.label, name.attributes
        )
    };
    DecisionOption::new(id, payload, summary)
}

impl DecisionSource for CampaignDecisions {
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError> {
        match self {
            Self::Explore(source) => source.choose(point),
            Self::Replay(source) => source.choose(point),
        }
    }
}

impl CampaignDecisions {
    fn finish(self) {
        if let Self::Replay(replay) = self {
            replay.finish().unwrap();
        }
    }
}

enum CampaignInput {
    Explore(TraceRecorder),
    Replay(DecisionTrace),
}

fn virtual_micros(tasks: &TaskRuntime) -> u64 {
    tasks.now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros().try_into().unwrap()
}

#[expect(clippy::too_many_arguments)]
async fn execute_action(
    action: CampaignAction,
    model: &mut CampaignModel,
    decisions: &mut dyn DecisionSource,
    producer: &Node,
    follower: &mut Option<Node>,
    follower_storage: &mut NodeStorage,
    served_blocks: &Arc<Mutex<BTreeMap<u64, SealedBlock<Block>>>>,
    tasks: &TaskRuntime,
    native: &reth_tasks::Runtime,
    campaign_config: NodeCampaignConfig,
) {
    match action {
        CampaignAction::BeginPayloadBuild { parent, branch } => {
            assert!(model.payload_build.is_none(), "payload build already active");
            let transaction_count = model.choose_transaction_count(decisions, tasks);
            let parent_block = model.blocks.get(&parent).unwrap();
            let build = producer
                .begin_build(
                    &parent_block.header,
                    parent_block.next_nonces,
                    branch,
                    transaction_count,
                )
                .await;
            assert_eq!(build.parent, parent);
            assert_eq!(build.branch, branch);
            model.payload_build = Some(build);
        }
        CampaignAction::PollPayloadBuild => {
            let build = model.payload_build.as_ref().expect("poll without an active payload build");
            let Some(payload) = producer.poll_build(build).await else { return };
            let build = model.payload_build.take().unwrap();
            let parent = build.parent;
            model.blocks.get_mut(&parent).unwrap().children += 1;
            let hash = payload.block().hash();
            assert!(model
                .pending
                .insert(
                    hash,
                    PendingPayload {
                        payload,
                        parent,
                        next_nonces: build.next_nonces,
                        corrupt_receipts_checked: false,
                        corrupt_state_checked: false,
                    },
                )
                .is_none());
        }
        CampaignAction::BeginStateRead { block } => {
            assert!(model.state_read.is_none(), "state read already active");
            let modeled = model.blocks.get(&block).expect("state read for unknown block");
            let provider = producer.provider.state_by_block_hash(block).unwrap();
            model.state_read = Some(StateRead {
                provider,
                block,
                state_root: modeled.header.state_root,
                nonces: modeled.next_nonces,
            });
        }
        CampaignAction::CompleteStateRead => {
            let read = model.state_read.take().expect("completion without an active state read");
            for (index, address) in workload_addresses().into_iter().enumerate() {
                let account = read.provider.basic_account(&address).unwrap().unwrap();
                assert_eq!(
                    account.nonce, read.nonces[index],
                    "retained state provider returned a torn account view for block {}",
                    read.block
                );
            }
            assert_eq!(
                read.provider.state_root(HashedPostState::default()).unwrap(),
                read.state_root,
                "retained state provider returned a torn trie view for block {}",
                read.block
            );
        }
        CampaignAction::ImportPayload { block } => {
            let pending = model.pending.remove(&block).expect("import without a pending payload");
            assert_eq!(pending.payload.block().hash(), block);
            let frontier_before = producer.sparse_trie_frontier();
            let parent_state_root = model.blocks[&pending.parent].header.state_root;
            producer.import(&pending.payload).await;
            if let Some((anchor_before, state_root_before)) = frontier_before &&
                state_root_before == parent_state_root &&
                model.blocks.contains_key(&anchor_before)
            {
                let (anchor_after, _) = producer
                    .sparse_trie_frontier()
                    .expect("import discarded the preserved sparse trie");
                assert!(
                    !model.is_strict_ancestor(anchor_after, anchor_before),
                    "reused sparse trie frontier moved backwards from {anchor_before} to {anchor_after}"
                );
                model.trie_frontier_reuses += 1;
            }
            let header = pending.payload.block().sealed_header().clone();
            model.blocks.insert(
                block,
                ModeledBlock {
                    header,
                    payload: Some(pending.payload),
                    parent: Some(pending.parent),
                    next_nonces: pending.next_nonces,
                    children: 0,
                    replayed: false,
                },
            );
            model.canonical_head =
                producer.provider.canonical_in_memory_state().get_canonical_head().hash();
        }
        CampaignAction::CorruptPayload { block, field } => {
            let pending =
                model.pending.get_mut(&block).expect("corruption without a pending payload");
            assert_eq!(pending.payload.block().hash(), block);
            let mut invalid = pending.payload.block().clone().into_block();
            match field {
                CorruptField::ReceiptsRoot => {
                    invalid.header.receipts_root = B256::repeat_byte(0x5a);
                    pending.corrupt_receipts_checked = true;
                }
                CorruptField::StateRoot => {
                    invalid.header.state_root = B256::repeat_byte(0xa5);
                    pending.corrupt_state_checked = true;
                }
            }
            let (payload, sidecar) =
                alloy_rpc_types_engine::ExecutionPayload::from_block_slow(&invalid);
            let status = producer.new_payload(ExecutionData::new(payload, sidecar)).await;
            assert!(status.is_invalid(), "mutated {field:?} payload was accepted: {status:?}");
        }
        CampaignAction::SetHead { head } => {
            assert!(model.blocks.contains_key(&head));
            assert!(producer.forkchoice(head).await.is_valid());
            model.canonical_head =
                producer.provider.canonical_in_memory_state().get_canonical_head().hash();
        }
        CampaignAction::BeginFollowerSync { head } => {
            assert!(model.follower_sync_target.is_none());
            assert!(!model.follower_partitioned);
            install_served_chain(model, head, served_blocks);
            let Some(status) = follower.as_ref().unwrap().try_forkchoice(head).await else {
                assert!(
                    model.database_faults.needs_recovery(),
                    "follower stopped without an injected database failure"
                );
                return
            };
            if status.is_valid() {
                model.follower_head = follower
                    .as_ref()
                    .unwrap()
                    .provider
                    .canonical_in_memory_state()
                    .get_canonical_head()
                    .hash();
            } else {
                assert!(status.is_syncing(), "follower rejected modeled chain: {status:?}");
                model.follower_sync_target = Some(head);
            }
        }
        CampaignAction::PollFollowerSync => {
            let head = model.follower_sync_target.expect("poll without an active follower sync");
            let Some(status) = follower.as_ref().unwrap().try_forkchoice(head).await else {
                assert!(
                    model.database_faults.needs_recovery(),
                    "follower stopped without an injected database failure"
                );
                return
            };
            if status.is_valid() {
                model.follower_sync_target = None;
                // Fork choice can validly retain a newer canonical head when `head` is an already
                // known ancestor. Follow the engine's observed state rather than assuming the
                // requested head caused a rewind.
                model.follower_head = follower
                    .as_ref()
                    .unwrap()
                    .provider
                    .canonical_in_memory_state()
                    .get_canonical_head()
                    .hash();
                let root = model.blocks.get(&model.follower_head).unwrap().header.state_root;
                follower.as_ref().unwrap().assert_sparse_root(root);
            } else {
                assert!(status.is_syncing(), "follower rejected modeled chain: {status:?}");
            }
        }
        CampaignAction::PartitionFollower => {
            assert!(!model.follower_partitioned);
            follower.as_ref().unwrap().peer.set_partitioned(true);
            model.follower_partitioned = true;
        }
        CampaignAction::HealFollower => {
            assert!(model.follower_partitioned);
            follower.as_ref().unwrap().peer.set_partitioned(false);
            model.follower_partitioned = false;
        }
        CampaignAction::AdvanceTime => {
            let duration = model.choose_time_advance(decisions, tasks);
            tasks.sleep(duration).await;
        }
        action @ (CampaignAction::CrashRestartFollower |
        CampaignAction::GracefulRestartFollower) => {
            let crash = matches!(action, CampaignAction::CrashRestartFollower);
            assert_eq!(
                crash, !campaign_config.native_workers,
                "native workers require graceful in-process restart"
            );
            model.database_faults.suppress();
            let durable = follower.as_ref().unwrap().provider.database_provider_ro().unwrap();
            let durable_number = durable.best_block_number().unwrap();
            let durable_head = durable.block_hash(durable_number).unwrap().unwrap();
            drop(durable);
            model.database_faults.resume();
            let old_peer = &follower.as_ref().unwrap().peer;
            model.follower_wire.extend(old_peer.trace());
            if crash {
                follower.take().unwrap().crash().await;
                native.spawn_blocking_named("crash-barrier", || ()).get();
            } else {
                follower.take().unwrap().shutdown().await;
            }
            let release_deadline = tasks.now() + Duration::from_secs(1);
            while follower_storage.is_open() {
                assert!(
                    tasks.now() < release_deadline,
                    "stopped follower did not release its database handles"
                );
                tasks.sleep(Duration::from_millis(1)).await;
            }
            let overlay = OverlayManager::new(native.state_trie_overlay_worker_pool());
            model.database_faults.suppress();
            let factory = follower_storage.open(overlay.clone(), native.clone());
            assert_eq!(factory.check_consistency().unwrap(), (None, None));
            model.follower_restarts += 1;
            let restart_seed = u64::from_be_bytes(
                model.state_digest().as_slice()[..8].try_into().expect("digest prefix"),
            );
            *follower = Some(
                Node::launch(
                    factory,
                    overlay,
                    Arc::clone(served_blocks),
                    tasks.clone(),
                    native.clone(),
                    restart_seed,
                    campaign_config,
                )
                .await,
            );
            model.database_faults.resume();
            if model.database_faults.needs_recovery() {
                model.database_faults.mark_recovered();
            }
            model.follower_head = durable_head;
            model.follower_sync_target = None;
            model.follower_partitioned = false;
        }
        CampaignAction::CorruptFollowerResponse => {
            follower.as_ref().unwrap().peer.corrupt_next_response();
        }
        CampaignAction::ReplayPayload { block } => {
            let payload = model.blocks[&block].payload.as_ref().unwrap().clone();
            let status = producer.new_payload(payload.into()).await;
            assert!(status.is_valid(), "replayed valid payload was rejected: {status:?}");
            model.blocks.get_mut(&block).unwrap().replayed = true;
        }
        CampaignAction::EnableDatabaseFault => model.database_faults.arm(),
    }

    let expected = &model.blocks.get(&model.canonical_head).unwrap().header;
    let observed = producer.provider.canonical_in_memory_state().get_canonical_head();
    assert_eq!(observed.num_hash(), expected.num_hash());
}

#[expect(clippy::too_many_arguments)]
async fn converge_follower(
    head: B256,
    model: &mut CampaignModel,
    decisions: &mut dyn DecisionSource,
    producer: &Node,
    follower: &mut Option<Node>,
    follower_storage: &mut NodeStorage,
    served_blocks: &Arc<Mutex<BTreeMap<u64, SealedBlock<Block>>>>,
    tasks: &TaskRuntime,
    native: &reth_tasks::Runtime,
    campaign_config: NodeCampaignConfig,
) {
    let deadline = tasks.now() + Duration::from_secs(5);
    loop {
        if model.database_faults.needs_recovery() {
            let restart_action = if campaign_config.native_workers {
                CampaignAction::GracefulRestartFollower
            } else {
                CampaignAction::CrashRestartFollower
            };
            let restart = model.choose(&mut *decisions, tasks, vec![restart_action]);
            execute_action(
                restart,
                model,
                decisions,
                producer,
                follower,
                follower_storage,
                served_blocks,
                tasks,
                native,
                campaign_config,
            )
            .await;
        }
        if !follower.as_ref().unwrap().peer.is_connected() {
            let restart_action = if campaign_config.native_workers {
                CampaignAction::GracefulRestartFollower
            } else {
                CampaignAction::CrashRestartFollower
            };
            let restart = model.choose(&mut *decisions, tasks, vec![restart_action]);
            execute_action(
                restart,
                model,
                decisions,
                producer,
                follower,
                follower_storage,
                served_blocks,
                tasks,
                native,
                campaign_config,
            )
            .await;
        }
        if model.follower_partitioned {
            let heal = model.choose(&mut *decisions, tasks, vec![CampaignAction::HealFollower]);
            execute_action(
                heal,
                model,
                decisions,
                producer,
                follower,
                follower_storage,
                served_blocks,
                tasks,
                native,
                campaign_config,
            )
            .await;
        }
        if model.follower_sync_target.is_none() {
            if model.follower_head == head {
                return
            }
            let begin = model.choose(
                &mut *decisions,
                tasks,
                vec![CampaignAction::BeginFollowerSync { head }],
            );
            execute_action(
                begin,
                model,
                decisions,
                producer,
                follower,
                follower_storage,
                served_blocks,
                tasks,
                native,
                campaign_config,
            )
            .await;
            if model.follower_sync_target.is_none() {
                return
            }
        }
        let advance = model.choose(&mut *decisions, tasks, vec![CampaignAction::AdvanceTime]);
        execute_action(
            advance,
            model,
            decisions,
            producer,
            follower,
            follower_storage,
            served_blocks,
            tasks,
            native,
            campaign_config,
        )
        .await;
        if !follower.as_ref().unwrap().peer.is_connected() {
            continue
        }
        let poll = model.choose(&mut *decisions, tasks, vec![CampaignAction::PollFollowerSync]);
        execute_action(
            poll,
            model,
            decisions,
            producer,
            follower,
            follower_storage,
            served_blocks,
            tasks,
            native,
            campaign_config,
        )
        .await;
        assert!(
            tasks.now() < deadline,
            "follower did not sync before the virtual deadline: target={head}, observed={:?}, wire={:?}",
            follower
                .as_ref()
                .unwrap()
                .provider
                .canonical_in_memory_state()
                .get_canonical_head()
                .num_hash(),
            follower.as_ref().unwrap().peer.trace(),
        );
    }
}

fn install_served_chain(
    model: &CampaignModel,
    head: B256,
    served_blocks: &Arc<Mutex<BTreeMap<u64, SealedBlock<Block>>>>,
) {
    let mut served = served_blocks.lock().unwrap();
    served.retain(|number, _| *number == 0);
    for hash in model.canonical_hashes(head).into_iter().skip(1) {
        let payload = model.blocks.get(&hash).unwrap().payload.as_ref().unwrap();
        served.insert(payload.block().number(), payload.block().clone());
    }
}

fn simulate_node(
    seed: u64,
    input: CampaignInput,
    action_budget: usize,
    decision_limit: usize,
) -> NodeOutcome {
    let decisions = SharedDecisions::new(input, seed, decision_limit);
    let virtual_time = Arc::new(AtomicU64::new(0));
    let campaign_config = NodeCampaignConfig::from_seed(seed);
    let follower_database_faults = CampaignDatabaseFaults::new(
        decisions.clone(),
        Arc::clone(&virtual_time),
        campaign_config.inject_database_fault,
    );
    let chain = node_chain();
    let native = reth_tasks::Runtime::test();
    let mut producer_storage = NodeStorage::new(chain.clone(), DatabaseFaults::disabled());
    let mut follower_storage = NodeStorage::new(chain.clone(), follower_database_faults.adapter());
    let producer_overlay = OverlayManager::new(native.state_trie_overlay_worker_pool());
    let producer_factory = producer_storage.open(producer_overlay.clone(), native.clone());
    let follower_overlay = OverlayManager::new(native.state_trie_overlay_worker_pool());
    let follower_factory = follower_storage.open(follower_overlay.clone(), native.clone());
    init_genesis_with_settings(&producer_factory, StorageSettings::v2()).unwrap();
    init_genesis_with_settings(&follower_factory, StorageSettings::v2()).unwrap();
    let genesis_block = producer_factory.block_by_number(0).unwrap().unwrap().seal_slow();
    let config = deterministic::Config::default()
        .with_seed(seed)
        .with_scheduling_policy(SemanticScheduler::new(decisions.clone(), virtual_time))
        // Several worker polls must fit inside the validator's 1ms speculative window. A 100µs
        // cycle also lets network request timeouts advance without millions of duplicate wakes.
        .with_cycle(Duration::from_micros(100))
        .with_timeout(Some(Duration::from_secs(120)));
    let run_decisions = decisions.clone();
    let run_database_faults = Arc::clone(&follower_database_faults);
    let outcome = deterministic::Runner::new(config).start(move |context| async move {
        let tasks = TaskRuntime::deterministic(context.child("nodes"));
        tasks
            .clone()
            .scope(async move {
                let mut decisions = run_decisions;
                let blocks = Arc::new(Mutex::new(BTreeMap::from([(0, genesis_block)])));
                let producer = Node::launch(
                    producer_factory,
                    producer_overlay,
                    blocks.clone(),
                    TaskRuntime::deterministic(context.child("producer")),
                    native.clone(),
                    seed,
                    campaign_config,
                )
                .await;
                let mut follower = Some(Node::launch(
                    follower_factory,
                    follower_overlay,
                    blocks.clone(),
                    TaskRuntime::deterministic(context.child("follower")),
                    native.clone(),
                    seed.wrapping_add(1),
                    campaign_config,
                )
                .await);
                let genesis = SealedHeader::seal_slow(chain.genesis_header().clone());
                let mut model = CampaignModel::new(
                    genesis,
                    action_budget.max(8),
                    Arc::clone(&run_database_faults),
                    !campaign_config.native_workers,
                );
                for _ in 0..action_budget {
                    let action = model.choose(&mut decisions, &tasks, model.legal_actions());
                    execute_action(
                        action,
                        &mut model,
                        &mut decisions,
                        &producer,
                        &mut follower,
                        &mut follower_storage,
                        &blocks,
                        &tasks,
                        &native,
                        campaign_config,
                    )
                    .await;
                }
                while model.payload_build.is_some() {
                    for action in [CampaignAction::AdvanceTime, CampaignAction::PollPayloadBuild] {
                        let action = model.choose(&mut decisions, &tasks, vec![action]);
                        execute_action(
                            action,
                            &mut model,
                            &mut decisions,
                            &producer,
                            &mut follower,
                            &mut follower_storage,
                            &blocks,
                            &tasks,
                            &native,
                            campaign_config,
                        )
                        .await;
                        if model.payload_build.is_none() {
                            break
                        }
                    }
                }
                while let Some(block) = model.pending.keys().next().copied() {
                    let action = CampaignAction::ImportPayload { block };
                    let action = model.choose(&mut decisions, &tasks, vec![action]);
                    execute_action(
                        action,
                        &mut model,
                        &mut decisions,
                        &producer,
                        &mut follower,
                        &mut follower_storage,
                        &blocks,
                        &tasks,
                        &native,
                        campaign_config,
                    )
                    .await;
                }
                if model.state_read.is_some() {
                    let action = model.choose(
                        &mut decisions,
                        &tasks,
                        vec![CampaignAction::CompleteStateRead],
                    );
                    execute_action(
                        action,
                        &mut model,
                        &mut decisions,
                        &producer,
                        &mut follower,
                        &mut follower_storage,
                        &blocks,
                        &tasks,
                        &native,
                        campaign_config,
                    )
                    .await;
                }
                if run_database_faults.can_arm() {
                    let action = model.choose(
                        &mut decisions,
                        &tasks,
                        vec![CampaignAction::EnableDatabaseFault],
                    );
                    execute_action(
                        action,
                        &mut model,
                        &mut decisions,
                        &producer,
                        &mut follower,
                        &mut follower_storage,
                        &blocks,
                        &tasks,
                        &native,
                        campaign_config,
                    )
                    .await;
                }
                let current_follower_target =
                    model.follower_sync_target.unwrap_or(model.canonical_head);
                converge_follower(
                    current_follower_target,
                    &mut model,
                    &mut decisions,
                    &producer,
                    &mut follower,
                    &mut follower_storage,
                    &blocks,
                    &tasks,
                    &native,
                    campaign_config,
                )
                .await;
                if campaign_config.inject_database_fault {
                    assert!(
                        !run_database_faults.needs_recovery(),
                        "database failure was not followed by a cold recovery"
                    );
                }
                run_database_faults.disarm();
                let head = model.deepest_head();
                if model.canonical_head != head {
                    let action = model.choose(
                        &mut decisions,
                        &tasks,
                        vec![CampaignAction::SetHead { head }],
                    );
                    execute_action(
                        action,
                        &mut model,
                        &mut decisions,
                        &producer,
                        &mut follower,
                        &mut follower_storage,
                        &blocks,
                        &tasks,
                        &native,
                        campaign_config,
                    )
                    .await;
                }
                converge_follower(
                    head,
                    &mut model,
                    &mut decisions,
                    &producer,
                    &mut follower,
                    &mut follower_storage,
                    &blocks,
                    &tasks,
                    &native,
                    campaign_config,
                )
                .await;

                let canonical = model.canonical_hashes(head);
                let canonical_without_genesis =
                    canonical.iter().copied().skip(1).collect::<Vec<_>>();
                let head_block = model.blocks.get(&head).unwrap();
                let head_number = head_block.header.number;
                let head_nonces = head_block.next_nonces;
                let head_children = head_block.children;
                assert_eq!(producer.provider.best_block_number().unwrap(), head_number);
                assert_eq!(
                    follower.as_ref().unwrap().provider.best_block_number().unwrap(),
                    head_number
                );
                for (number, hash) in canonical.iter().enumerate() {
                    assert_eq!(
                        follower
                            .as_ref()
                            .unwrap()
                            .provider
                            .block_hash(number as u64)
                            .unwrap(),
                        Some(*hash)
                    );
                }

                // The newest `persistence_threshold` blocks may remain in memory. Waiting for the
                // older prefix proves the live persistence worker ran before cold restart.
                let durable_target =
                    head_number.saturating_sub(campaign_config.persistence_threshold);
                let persistence_deadline = tasks.now() + Duration::from_secs(5);
                loop {
                    let producer_tip = producer
                        .provider
                        .database_provider_ro()
                        .unwrap()
                        .best_block_number()
                        .unwrap();
                    let follower_tip = follower
                        .as_ref()
                        .unwrap()
                        .provider
                        .database_provider_ro()
                        .unwrap()
                        .best_block_number()
                        .unwrap();
                    if producer_tip >= durable_target && follower_tip >= durable_target {
                        break;
                    }
                    assert!(
                        tasks.now() < persistence_deadline,
                        "persistence did not reach block {durable_target} before the virtual deadline: producer={producer_tip}, follower={follower_tip}"
                    );
                    tasks.sleep(Duration::from_millis(1)).await;
                }
                for (index, address) in workload_addresses().into_iter().enumerate() {
                    let producer_account =
                        producer.provider.latest().unwrap().basic_account(&address).unwrap();
                    let follower_account = follower
                        .as_ref()
                        .unwrap()
                        .provider
                        .latest()
                        .unwrap()
                        .basic_account(&address)
                        .unwrap();
                    assert_eq!(producer_account, follower_account);
                    assert_eq!(producer_account.unwrap().nonce, head_nonces[index]);
                }
                let first_block = model.first_block();
                let peer = &follower.as_ref().unwrap().peer;
                let mut wire = std::mem::take(&mut model.follower_wire);
                wire.extend(peer.trace());
                // The modeled peer can switch forks while requests from the previous sync are
                // still in flight. The downloader may report those internally consistent but
                // cross-fork batches; successful convergence is checked above.
                let producer_prewarming = Arc::clone(&producer.prewarming);
                let follower_prewarming = Arc::clone(&follower.as_ref().unwrap().prewarming);
                producer.shutdown().await;
                follower.take().unwrap().shutdown().await;
                model.record_event(
                    &mut decisions,
                    &tasks,
                    DecisionDomain::NodeLifecycle,
                    head.as_slice().to_vec(),
                    "cold restart follower",
                );
                let restart_overlay = OverlayManager::new(native.state_trie_overlay_worker_pool());
                let restart_factory =
                    follower_storage.open(restart_overlay.clone(), native.clone());
                assert_eq!(restart_factory.check_consistency().unwrap(), (None, None));
                let restarted = Node::launch(
                    restart_factory,
                    restart_overlay,
                    blocks,
                    TaskRuntime::deterministic(context.child("restarted")),
                    native.clone(),
                    seed.wrapping_add(2),
                    campaign_config,
                )
                .await;
                assert_eq!(restarted.provider.best_block_number().unwrap(), head_number);

                let branch = head_children;
                let request = CampaignAction::BeginPayloadBuild { parent: head, branch };
                let request = model.choose(&mut decisions, &tasks, vec![request]);
                let CampaignAction::BeginPayloadBuild { parent, branch } = request else {
                    unreachable!()
                };
                let transaction_count = model.choose_transaction_count(&mut decisions, &tasks);
                let parent = model.blocks.get(&parent).unwrap();
                let (payload, final_nonces) = restarted
                    .build(
                        &parent.header,
                        parent.next_nonces,
                        branch,
                        transaction_count,
                    )
                    .await;
                let final_hash = payload.block().hash();
                model.blocks.get_mut(&head).unwrap().children += 1;
                model.pending.insert(
                    final_hash,
                    PendingPayload {
                        payload,
                        parent: head,
                        next_nonces: final_nonces,
                        corrupt_receipts_checked: false,
                        corrupt_state_checked: false,
                    },
                );
                let import = model.choose(
                    &mut decisions,
                    &tasks,
                    vec![CampaignAction::ImportPayload { block: final_hash }],
                );
                let CampaignAction::ImportPayload { .. } = import else { unreachable!() };
                let pending = model.pending.remove(&final_hash).unwrap();
                restarted.import(&pending.payload).await;
                model.blocks.insert(
                    final_hash,
                    ModeledBlock {
                        header: pending.payload.block().sealed_header().clone(),
                        payload: Some(pending.payload),
                        parent: Some(head),
                        next_nonces: final_nonces,
                        children: 0,
                        replayed: false,
                    },
                );
                model.canonical_head = final_hash;
                let restarted_prewarming = Arc::clone(&restarted.prewarming);
                restarted.shutdown().await;
                let final_factory = follower_storage
                    .open(OverlayManager::new(native.state_trie_overlay_worker_pool()), native);
                assert_eq!(final_factory.check_consistency().unwrap(), (None, None));
                let persisted = BlockchainProvider::new(final_factory).unwrap();
                let mut accounts = Vec::with_capacity(WORKLOAD_ACCOUNT_COUNT);
                for (index, address) in workload_addresses().into_iter().enumerate() {
                    let account =
                        persisted.latest().unwrap().basic_account(&address).unwrap().unwrap();
                    assert_eq!(account.nonce, final_nonces[index]);
                    accounts.push((address, account.nonce, account.balance));
                }
                let final_number = head_number + 1;
                assert_eq!(persisted.best_block_number().unwrap(), final_number);
                assert_eq!(persisted.block_hash(final_number).unwrap(), Some(final_hash));
                let persisted_root = persisted
                    .latest()
                    .unwrap()
                    .state_root(HashedPostState::default())
                    .unwrap();
                assert_eq!(persisted_root, model.blocks[&final_hash].header.state_root);
                let prewarmed_transactions =
                    [producer_prewarming, follower_prewarming, restarted_prewarming]
                        .map(|counter| counter.load(std::sync::atomic::Ordering::Relaxed));
                NodeOutcome {
                    audit: context.auditor().state(),
                    canonical: canonical_without_genesis,
                    first_block,
                    wire,
                    persisted_head: final_hash,
                    accounts,
                    prewarmed_transactions,
                    trie_frontier_reuses: model.trie_frontier_reuses,
                }
            })
            .await
    });
    drop(producer_storage);
    drop(follower_database_faults);
    decisions.finish();
    outcome
}

fn campaign_trace_header(seed: u64, action_budget: usize, decision_limit: usize) -> TraceHeader {
    let chain = node_chain();
    let genesis = SealedHeader::seal_slow(chain.genesis_header().clone());
    let campaign_config = NodeCampaignConfig::from_seed(seed);
    let configuration = keccak256(
        [
            CAMPAIGN_SCHEMA_VERSION.to_be_bytes().as_slice(),
            seed.to_be_bytes().as_slice(),
            (action_budget as u64).to_be_bytes().as_slice(),
            (MIN_TRANSACTIONS_PER_BLOCK as u64).to_be_bytes().as_slice(),
            (MAX_TRANSACTIONS_PER_BLOCK as u64).to_be_bytes().as_slice(),
            (decision_limit as u64).to_be_bytes().as_slice(),
            campaign_config.persistence_threshold.to_be_bytes().as_slice(),
            campaign_config.state_masking_blocks.to_be_bytes().as_slice(),
            (campaign_config.multiproof_chunk_size as u64).to_be_bytes().as_slice(),
            [u8::from(campaign_config.inject_database_fault)].as_slice(),
            [u8::from(campaign_config.native_workers)].as_slice(),
        ]
        .concat(),
    );
    TraceHeader::new(env!("CARGO_PKG_VERSION"), configuration, genesis.hash())
        .with_campaign(seed, action_budget as u64)
}

pub(crate) fn run_node_campaign() {
    if std::env::var_os("RETH_DST_NATIVE_WORKERS").is_some() {
        assert!(
            cfg!(feature = "native-differential"),
            "RETH_DST_NATIVE_WORKERS requires --features dst,native-differential"
        );
        reth_rayon::force_native();
    }
    if let Ok(path) = std::env::var("RETH_DST_REPLAY") {
        let trace = DecisionTrace::decode(&reth_fs_util::read(path).unwrap()).unwrap();
        let seed = trace.header.exploration_seed.expect("trace has no exploration seed");
        let action_budget = trace.header.decision_budget.expect("trace has no decision budget");
        let decision_limit = std::env::var("RETH_DST_MAX_DECISIONS")
            .map(|limit| limit.parse().expect("RETH_DST_MAX_DECISIONS must be an integer"))
            .unwrap_or_else(|_| trace.decisions.len().saturating_add(1));
        simulate_node(seed, CampaignInput::Replay(trace), action_budget as usize, decision_limit);
        return
    }

    let first_seed: u64 = std::env::var("RETH_DST_SEED")
        .map(|seed| seed.parse().expect("RETH_DST_SEED must be a u64"))
        .unwrap_or(0);
    let campaign_seconds = std::env::var("RETH_DST_SECONDS")
        .ok()
        .map(|seconds| seconds.parse().expect("RETH_DST_SECONDS must be a u64"));
    let case_limit = std::env::var("RETH_DST_CASES")
        .map(|cases| cases.parse().expect("RETH_DST_CASES must be an integer"))
        .unwrap_or_else(|_| {
            if campaign_seconds.is_some() {
                usize::MAX
            } else if std::env::var_os("RETH_DST_SEED").is_some() {
                1
            } else {
                4
            }
        });
    let host_deadline =
        campaign_seconds.map(|seconds| std::time::Instant::now() + Duration::from_secs(seconds));
    let action_budget: usize = std::env::var("RETH_DST_STEPS")
        .map(|steps| steps.parse().expect("RETH_DST_STEPS must be an integer"))
        .unwrap_or(16);
    let decision_limit = std::env::var("RETH_DST_MAX_DECISIONS")
        .map(|limit| limit.parse().expect("RETH_DST_MAX_DECISIONS must be an integer"))
        .unwrap_or_else(|_| action_budget.saturating_mul(128).max(8_192));
    let mut failures = 0;
    let mut index = 0;
    while index < case_limit &&
        host_deadline.is_none_or(|deadline| std::time::Instant::now() < deadline)
    {
        let seed = first_seed.wrapping_add(index as u64);
        let recorder =
            TraceRecorder::new(campaign_trace_header(seed, action_budget, decision_limit));
        let watchdog = CaseWatchdog::spawn(recorder.clone(), seed);
        let explored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            simulate_node(
                seed,
                CampaignInput::Explore(recorder.clone()),
                action_budget,
                decision_limit,
            )
        }));
        drop(watchdog);
        let outcome = match explored {
            Ok(outcome) => outcome,
            Err(payload) => {
                let trace = recorder.snapshot();
                if let Some(limit) = payload.downcast_ref::<DecisionLimitExceeded>() {
                    let artifact = write_trace("inconclusive", seed, &trace);
                    eprintln!(
                        "INCONCLUSIVE seed={seed} decisions={} trace={} reason=decision limit reached",
                        limit.selected,
                        artifact.display()
                    );
                    index += 1;
                    continue
                }
                let artifact = write_failure_trace(seed, &trace);
                eprintln!(
                    "BUG seed={seed} decisions={} trace={} panic={}",
                    trace.decisions.len(),
                    artifact.display(),
                    panic_message(payload.as_ref())
                );
                failures += 1;
                index += 1;
                continue
            }
        };
        let trace = recorder.snapshot();
        let storage_decisions = trace
            .decisions
            .iter()
            .filter(|decision| decision.point.domain == DecisionDomain::Storage)
            .count();
        let storage_job_schedules = trace
            .decisions
            .iter()
            .filter(|decision| {
                decision.point.domain == DecisionDomain::Schedule &&
                    decision.summary.contains("trie-storage-jobs")
            })
            .count();
        let injected_database_faults = trace
            .decisions
            .iter()
            .filter(|decision| decision.summary.starts_with("return database I/O error"))
            .map(|decision| decision.summary.as_str())
            .collect::<Vec<_>>();
        eprintln!(
            "node seed={seed} decisions={} storage_decisions={storage_decisions} storage_job_schedules={storage_job_schedules} trie_frontier_reuses={} database_faults={injected_database_faults:?}",
            trace.decisions.len(),
            outcome.trie_frontier_reuses,
        );
        if std::env::var_os("RETH_DST_NATIVE_WORKERS").is_some() {
            // External Tokio/Rayon completion timing is intentionally uncontrolled. This mode
            // is a product-worker differential, not a strict schedule-replay backend.
            index += 1;
            continue
        }
        if std::env::var_os("RETH_DST_VERIFY_PASSES").is_none() {
            // Campaign throughput is spent on new cases. Successful traces can be sampled for
            // strict replay by setting RETH_DST_VERIFY_PASSES; failures are retained for replay.
            index += 1;
            continue
        }
        let replay =
            simulate_node(seed, CampaignInput::Replay(trace), action_budget, decision_limit);
        assert_eq!(outcome, replay, "semantic replay diverged for seed {seed}");
        index += 1;
    }
    assert_eq!(failures, 0, "{failures} deterministic node campaign(s) failed");
}

fn write_failure_trace(seed: u64, trace: &DecisionTrace) -> std::path::PathBuf {
    write_trace("failures", seed, trace)
}

struct CaseWatchdog {
    done: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaseWatchdog {
    fn spawn(recorder: TraceRecorder, seed: u64) -> Self {
        let timeout_seconds: u64 = std::env::var("RETH_DST_CASE_TIMEOUT_SECS")
            .map(|seconds| seconds.parse().expect("RETH_DST_CASE_TIMEOUT_SECS must be a u64"))
            .unwrap_or(60);
        let (done, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("reth-dst-watchdog".into())
            .spawn(move || {
                if receiver.recv_timeout(Duration::from_secs(timeout_seconds)).is_err() {
                    let trace = recorder.snapshot();
                    let artifact = write_trace("inconclusive", seed, &trace);
                    eprintln!(
                        "INCONCLUSIVE seed={seed} decisions={} trace={} reason=host timeout",
                        trace.decisions.len(),
                        artifact.display()
                    );
                    std::process::exit(124);
                }
            })
            .expect("spawn DST case watchdog");
        Self { done, thread: Some(thread) }
    }
}

impl Drop for CaseWatchdog {
    fn drop(&mut self) {
        let _ = self.done.send(());
        self.thread.take().unwrap().join().expect("DST watchdog panicked");
    }
}

fn write_trace(kind: &str, seed: u64, trace: &DecisionTrace) -> std::path::PathBuf {
    let bytes = trace.encode().unwrap();
    let digest = keccak256(&bytes);
    let directory = std::env::var_os("RETH_DST_ARTIFACT_DIR")
        .map(|directory| std::path::PathBuf::from(directory).join(kind))
        .unwrap_or_else(|| std::path::PathBuf::from("target/reth-dst").join(kind));
    reth_fs_util::create_dir_all(&directory).unwrap();
    let name = format!(
        "node-{seed}-{:02x}{:02x}{:02x}{:02x}.dst",
        digest[0], digest[1], digest[2], digest[3]
    );
    let path = directory.join(name);
    reth_fs_util::write(&path, bytes).unwrap();
    path
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&'static str>().copied())
        .unwrap_or("non-string panic")
}
