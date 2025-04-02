use crate::{EthApi, EthApiBuilder};
use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpec, ChainSpecProvider};
use reth_consensus::test_utils::TestConsensus;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_evm::test_utils::MockExecutorProvider;
use reth_evm_ethereum::EthEvmConfig;
use reth_network::{EthNetworkPrimitives, NetworkHandle};
use reth_network_api::noop::NoopNetwork;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeTypes};
use reth_payload_builder::noop::NoopPayloadBuilderService;
use reth_storage_api::{noop::NoopProvider, BlockReader, BlockReaderIdExt, StateProviderFactory};
use reth_tasks::{TaskExecutor, TaskManager};
use reth_transaction_pool::test_utils::{testing_pool, TestPool};
use std::sync::Arc;

#[derive(Debug, Clone)]
struct TestDeps {
    pub pool: TestPool,
    pub evm: EthEvmConfig,
    pub executor: MockExecutorProvider,
    pub consensus: Arc<TestConsensus>,
    pub network: NetworkHandle<EthNetworkPrimitives>,
    pub provider: NoopProvider,
}

impl FullNodeTypes for TestDeps {
    type Types = ();
    type DB = ();
    type Provider = NoopProvider;
}

impl FullNodeComponents for TestDeps {
    type Pool = TestPool;
    type Evm = EthEvmConfig;
    type Executor = MockExecutorProvider;
    type Consensus = Arc<TestConsensus>;
    type Network = NetworkHandle<EthNetworkPrimitives>;

    fn pool(&self) -> &Self::Pool {
        &self.pool
    }

    fn evm_config(&self) -> &Self::Evm {
        &self.evm
    }

    fn block_executor(&self) -> &Self::Executor {
        unimplemented!("Unexpected call to block_executor")
    }

    fn consensus(&self) -> &Self::Consensus {
        unimplemented!("Unexpected call to consensus")
    }

    fn network(&self) -> &Self::Network {
        &self.network
    }

    fn payload_builder_handle(
        &self,
    ) -> &reth_payload_builder::PayloadBuilderHandle<<Self::Types as NodeTypes>::Payload> {
        unimplemented!("Unexpected call to payload_builder_handle")
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    fn task_executor(&self) -> &TaskExecutor {
        unimplemented!("Unexpected call to task_executor")
    }
}

fn build_test_eth_api<
    P: BlockReaderIdExt<
            Block = reth_ethereum_primitives::Block,
            Receipt = reth_ethereum_primitives::Receipt,
            Header = alloy_consensus::Header,
        > + BlockReader
        + ChainSpecProvider<ChainSpec = ChainSpec>
        + StateProviderFactory
        + CanonStateSubscriptions<Primitives = reth_ethereum_primitives::EthPrimitives>
        + Unpin
        + Clone
        + 'static,
>(
    provider: P,
) -> EthApi<P, TestPool, NoopNetwork, EthEvmConfig> {
    let provider = provider.clone();
    let transaction_pool = testing_pool();
    let evm = EthEvmConfig::new(provider.chain_spec());
    let executor = MockExecutorProvider::default();
    let consensus = Arc::new(TestConsensus::default());
    let tasks = TaskManager::current();
    let task_executor = tasks.executor();
    let (_, payload_builder_handle) = NoopPayloadBuilderService::<EthEngineTypes>::new();

    EthApiBuilder::new(
        provider.clone(),
        testing_pool(),
        NoopNetwork::default(),
        EthEvmConfig::new(provider.chain_spec()),
    )
    .build()
}
