use alloy_consensus::constants::MAINNET_GENESIS_HASH;
use alloy_eips::{eip1559::BaseFeeParams, eip6110::MAINNET_DEPOSIT_CONTRACT_ADDRESS};
use alloy_primitives::{b256, U256};
use reth_chainspec::{
    make_genesis_header, BaseFeeParamsKind, Chain, ChainSpec, DepositContract, EthereumHardfork,
    HardforkBlobParams, MAINNET_PRUNE_DELETE_LIMIT,
};
use reth_consensus::{test_utils::TestConsensus, ConsensusError, FullConsensus};
use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
use reth_db_common::init::init_genesis;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{execute::BlockExecutorProvider, test_utils::MockExecutorProvider, ConfigureEvm};
use reth_evm_ethereum::EthEvmConfig;
use reth_network::{
    config::SecretKey, p2p::BlockClient, NetworkConfigBuilder, NetworkHandle, NetworkManager,
    NetworkPrimitives,
};
use reth_network_api::FullNetwork;
use reth_node_api::{
    BlockTy, BodyTy, FullNodeComponents, FullNodeTypes, FullNodeTypesAdapter, HeaderTy, NodeTypes,
    NodeTypesWithDBAdapter, PrimitivesTy, TxTy,
};
use reth_payload_builder::{noop::NoopPayloadBuilderService, PayloadBuilderHandle};
use reth_primitives_traits::SealedHeader;
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    ProviderFactory,
};
use reth_storage_api::EthStorage;
use reth_tasks::{TaskExecutor, TaskManager};
use reth_transaction_pool::{test_utils::testing_pool, PoolTransaction, TransactionPool};
use std::{
    fmt::Debug,
    sync::{Arc, LazyLock},
};

pub async fn create_components() -> eyre::Result<impl FullNodeComponents> {
    let chain_spec = MAINNET.clone();
    let transaction_pool = testing_pool();
    let evm_config = EthEvmConfig::new(chain_spec.clone());
    let executor = MockExecutorProvider::default();
    let consensus = Arc::new(TestConsensus::default());

    let (static_dir, _) = create_test_static_files_dir();
    let db = create_test_rw_db();
    let provider_factory = ProviderFactory::<NodeTypesWithDBAdapter<TestNode, _>>::new(
        db,
        chain_spec.clone(),
        StaticFileProvider::read_write(static_dir.into_path()).expect("static file provider"),
    );

    let _genesis_hash = init_genesis(&provider_factory)?;
    let provider = BlockchainProvider::new(provider_factory.clone())?;

    let network_manager = NetworkManager::new(
        NetworkConfigBuilder::new(SecretKey::new(&mut rand::thread_rng()))
            .with_unused_discovery_port()
            .with_unused_listener_port()
            .build(provider_factory.clone()),
    )
    .await?;
    let network = network_manager.handle().clone();
    let tasks = TaskManager::current();
    let task_executor = tasks.executor();
    tasks.executor().spawn(network_manager);

    let (_, payload_builder_handle) = NoopPayloadBuilderService::<EthEngineTypes>::new();

    let components = NodeAdapter::<FullNodeTypesAdapter<_, _, _>, _> {
        components: Components {
            transaction_pool,
            evm_config,
            executor,
            consensus,
            network,
            payload_builder_handle,
        },
        task_executor,
        provider,
    };

    Ok(components)
}

/// The Ethereum mainnet spec
pub static MAINNET: LazyLock<Arc<ChainSpec>> = LazyLock::new(|| {
    let genesis =
        serde_json::from_str(include_str!("../../../../chainspec/res/genesis/mainnet.json"))
            .expect("Can't deserialize Mainnet genesis json");
    let hardforks = EthereumHardfork::mainnet().into();
    let mut spec = ChainSpec {
        chain: Chain::mainnet(),
        genesis_header: SealedHeader::new(
            make_genesis_header(&genesis, &hardforks),
            MAINNET_GENESIS_HASH,
        ),
        genesis,
        // <https://etherscan.io/block/15537394>
        paris_block_and_final_difficulty: Some((
            15537394,
            U256::from(58_750_003_716_598_352_816_469u128),
        )),
        hardforks,
        // https://etherscan.io/tx/0xe75fb554e433e03763a1560646ee22dcb74e5274b34c5ad644e7c0f619a7e1d0
        deposit_contract: Some(DepositContract::new(
            MAINNET_DEPOSIT_CONTRACT_ADDRESS,
            11052984,
            b256!("0x649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: MAINNET_PRUNE_DELETE_LIMIT,
        blob_params: HardforkBlobParams::default(),
    };
    spec.genesis.config.dao_fork_support = true;
    spec.into()
});

/// Container for the node's types and the components and other internals that can be used by
/// addons of the node.
#[derive(Debug)]
pub struct NodeAdapter<T: FullNodeTypes, C: NodeComponents<T>> {
    /// The components of the node.
    pub components: C,
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The provider of the node.
    pub provider: T::Provider,
}

/// An abstraction over the components of a node, consisting of:
///  - evm and executor
///  - transaction pool
///  - network
///  - payload builder.
pub trait NodeComponents<T: FullNodeTypes>: Clone + Debug + Unpin + Send + Sync + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<T::Types>>> + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Primitives = <T::Types as NodeTypes>::Primitives>;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider<Primitives = <T::Types as NodeTypes>::Primitives>;

    /// The consensus type of the node.
    type Consensus: FullConsensus<<T::Types as NodeTypes>::Primitives, Error = ConsensusError>
        + Clone
        + Unpin
        + 'static;

    /// Network API.
    type Network: FullNetwork<Client: BlockClient<Block = BlockTy<T::Types>>>;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the node's executor type.
    fn block_executor(&self) -> &Self::Executor;

    /// Returns the node's consensus type.
    fn consensus(&self) -> &Self::Consensus;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the handle to the payload builder service handling payload building requests from
    /// the engine.
    fn payload_builder_handle(&self) -> &PayloadBuilderHandle<<T::Types as NodeTypes>::Payload>;
}

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct Components<Node: FullNodeTypes, N: NetworkPrimitives, Pool, EVM, Executor, Consensus> {
    /// The transaction pool of the node.
    pub transaction_pool: Pool,
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    pub evm_config: EVM,
    /// The node's executor type used to execute individual blocks and batches of blocks.
    pub executor: Executor,
    /// The consensus implementation of the node.
    pub consensus: Consensus,
    /// The network implementation of the node.
    pub network: NetworkHandle<N>,
    /// The handle to the payload builder service.
    pub payload_builder_handle: PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>,
}

impl<Node, Pool, EVM, Executor, Cons, N> NodeComponents<Node>
    for Components<Node, N, Pool, EVM, Executor, Cons>
where
    Node: FullNodeTypes,
    N: NetworkPrimitives<
        BlockHeader = HeaderTy<Node::Types>,
        BlockBody = BodyTy<Node::Types>,
        Block = BlockTy<Node::Types>,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    EVM: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>> + 'static,
    Executor: BlockExecutorProvider<Primitives = PrimitivesTy<Node::Types>>,
    Cons:
        FullConsensus<PrimitivesTy<Node::Types>, Error = ConsensusError> + Clone + Unpin + 'static,
{
    type Pool = Pool;
    type Evm = EVM;
    type Executor = Executor;
    type Consensus = Cons;
    type Network = NetworkHandle<N>;

    fn pool(&self) -> &Self::Pool {
        &self.transaction_pool
    }

    fn evm_config(&self) -> &Self::Evm {
        &self.evm_config
    }

    fn block_executor(&self) -> &Self::Executor {
        &self.executor
    }

    fn consensus(&self) -> &Self::Consensus {
        &self.consensus
    }

    fn network(&self) -> &Self::Network {
        &self.network
    }

    fn payload_builder_handle(&self) -> &PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload> {
        &self.payload_builder_handle
    }
}

impl<Node, N, Pool, EVM, Executor, Cons> Clone for Components<Node, N, Pool, EVM, Executor, Cons>
where
    N: NetworkPrimitives,
    Node: FullNodeTypes,
    Pool: TransactionPool,
    EVM: ConfigureEvm,
    Executor: BlockExecutorProvider,
    Cons: Clone,
{
    fn clone(&self) -> Self {
        Self {
            transaction_pool: self.transaction_pool.clone(),
            evm_config: self.evm_config.clone(),
            executor: self.executor.clone(),
            consensus: self.consensus.clone(),
            network: self.network.clone(),
            payload_builder_handle: self.payload_builder_handle.clone(),
        }
    }
}

/// A test [`Node`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestNode;

impl NodeTypes for TestNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = reth_trie_db::MerklePatriciaTrie;
    type Storage = EthStorage;
    type Payload = EthEngineTypes;
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeTypes for NodeAdapter<T, C> {
    type Types = T::Types;
    type DB = T::DB;
    type Provider = T::Provider;
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeComponents for NodeAdapter<T, C> {
    type Pool = C::Pool;
    type Evm = C::Evm;
    type Executor = C::Executor;
    type Consensus = C::Consensus;
    type Network = C::Network;

    fn pool(&self) -> &Self::Pool {
        self.components.pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.components.evm_config()
    }

    fn block_executor(&self) -> &Self::Executor {
        self.components.block_executor()
    }

    fn consensus(&self) -> &Self::Consensus {
        self.components.consensus()
    }

    fn network(&self) -> &Self::Network {
        self.components.network()
    }

    fn payload_builder_handle(
        &self,
    ) -> &reth_payload_builder::PayloadBuilderHandle<
        <Self::Types as reth_node_api::NodeTypes>::Payload,
    > {
        self.components.payload_builder_handle()
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    fn task_executor(&self) -> &TaskExecutor {
        &self.task_executor
    }
}

impl<T: FullNodeTypes, C: NodeComponents<T>> Clone for NodeAdapter<T, C> {
    fn clone(&self) -> Self {
        Self {
            components: self.components.clone(),
            task_executor: self.task_executor.clone(),
            provider: self.provider.clone(),
        }
    }
}
