//! Helper trait for interfacing with node components.

use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks};
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_primitives_traits::{BlockTy, HeaderTy, NodePrimitives, ReceiptTy, TxTy};
use reth_rpc_eth_types::EthStateCache;
use reth_storage_api::{
    BalProvider, BlockReader, BlockReaderIdExt, NodePrimitivesProvider, PruneCheckpointReader,
    StageCheckpointReader, StateProviderFactory,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// Helper trait that exposes the node components needed by RPC types: the provider, pool, network
/// and EVM config.
///
/// Unlike a full node components trait, it doesn't enforce the bounds required to run a node. This
/// makes it useful for RPC types that only need these components. Use [`RpcNodeCoreAdapter`] to
/// construct an implementation from the components.
pub trait RpcNodeCore: Clone + Send + Sync + Unpin + 'static {
    /// Blockchain data primitives.
    type Primitives: NodePrimitives;
    /// The provider type used to interact with the node.
    type Provider: BlockReaderIdExt<
            Block = BlockTy<Self::Primitives>,
            Receipt = ReceiptTy<Self::Primitives>,
            Header = HeaderTy<Self::Primitives>,
            Transaction = TxTy<Self::Primitives>,
        > + ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = HeaderTy<Self::Primitives>>
                           + Hardforks
                           + EthereumHardforks,
        > + StateProviderFactory<Primitives = Self::Primitives>
        + NodePrimitivesProvider<Primitives = Self::Primitives>
        + CanonStateSubscriptions<Primitives = Self::Primitives>
        + StageCheckpointReader
        + PruneCheckpointReader
        + BalProvider
        + Clone
        + Unpin
        + 'static;
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Self::Primitives>>>;
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Primitives = Self::Primitives> + 'static;
    /// Network API.
    type Network: NetworkInfo + Clone;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;
}

/// Additional components, asides the core node components, needed to run `eth_` namespace API
/// server.
pub trait RpcNodeCoreExt: RpcNodeCore<Provider: BlockReader> {
    /// Returns handle to RPC cache service.
    fn cache(&self) -> &EthStateCache<Self::Primitives>;
}

/// An adapter that allows to construct [`RpcNodeCore`] from components.
#[derive(Debug, Clone)]
pub struct RpcNodeCoreAdapter<Provider, Pool, Network, Evm> {
    provider: Provider,
    pool: Pool,
    network: Network,
    evm_config: Evm,
}

impl<Provider, Pool, Network, Evm> RpcNodeCoreAdapter<Provider, Pool, Network, Evm> {
    /// Creates a new `RpcNodeCoreAdapter` instance.
    pub const fn new(provider: Provider, pool: Pool, network: Network, evm_config: Evm) -> Self {
        Self { provider, pool, network, evm_config }
    }
}

impl<Provider, Pool, Network, Evm> RpcNodeCore for RpcNodeCoreAdapter<Provider, Pool, Network, Evm>
where
    Provider: BlockReaderIdExt<
            Block = BlockTy<Evm::Primitives>,
            Receipt = ReceiptTy<Evm::Primitives>,
            Header = HeaderTy<Evm::Primitives>,
            Transaction = TxTy<Evm::Primitives>,
        > + ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = HeaderTy<Evm::Primitives>>
                           + Hardforks
                           + EthereumHardforks,
        > + StateProviderFactory<Primitives = Evm::Primitives>
        + NodePrimitivesProvider<Primitives = Evm::Primitives>
        + CanonStateSubscriptions<Primitives = Evm::Primitives>
        + StageCheckpointReader
        + PruneCheckpointReader
        + BalProvider
        + Unpin
        + Clone
        + 'static,
    Evm: ConfigureEvm + 'static,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Evm::Primitives>>>
        + Unpin
        + 'static,
    Network: NetworkInfo + Clone + Unpin + 'static,
{
    type Primitives = Evm::Primitives;
    type Provider = Provider;
    type Pool = Pool;
    type Evm = Evm;
    type Network = Network;

    fn pool(&self) -> &Self::Pool {
        &self.pool
    }

    fn evm_config(&self) -> &Self::Evm {
        &self.evm_config
    }

    fn network(&self) -> &Self::Network {
        &self.network
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }
}
