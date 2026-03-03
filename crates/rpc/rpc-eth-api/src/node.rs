//! Helper trait for interfacing with [`FullNodeComponents`].

use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks};
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_node_api::{FullNodeComponents, NodePrimitives, PrimitivesTy};
use reth_primitives_traits::{BlockTy, HeaderTy, ReceiptTy, TxTy};
use reth_rpc_eth_types::EthStateCache;
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, StageCheckpointReader, StateProviderFactory,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// Helper trait that provides the same interface as [`FullNodeComponents`] but without requiring
/// implementation of trait bounds.
///
/// This trait is structurally equivalent to [`FullNodeComponents`], exposing the same associated
/// types and methods. However, it doesn't enforce the trait bounds required by
/// [`FullNodeComponents`]. This makes it useful for RPC types that need access to node components
/// where the full trait bounds of the components are not necessary.
///
/// Every type that is a [`FullNodeComponents`] also implements this trait.
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
        > + StateProviderFactory
        + CanonStateSubscriptions<Primitives = Self::Primitives>
        + StageCheckpointReader
        + Send
        + Sync
        + Clone
        + Unpin
        + 'static;
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Self::Primitives>>>;
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Primitives = Self::Primitives> + Send + Sync + 'static;
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

impl<T> RpcNodeCore for T
where
    T: FullNodeComponents<Provider: ChainSpecProvider<ChainSpec: Hardforks + EthereumHardforks>>,
{
    type Primitives = PrimitivesTy<T::Types>;
    type Provider = T::Provider;
    type Pool = T::Pool;
    type Evm = T::Evm;
    type Network = T::Network;

    #[inline]
    fn pool(&self) -> &Self::Pool {
        FullNodeComponents::pool(self)
    }

    #[inline]
    fn evm_config(&self) -> &Self::Evm {
        FullNodeComponents::evm_config(self)
    }

    #[inline]
    fn network(&self) -> &Self::Network {
        FullNodeComponents::network(self)
    }

    #[inline]
    fn provider(&self) -> &Self::Provider {
        FullNodeComponents::provider(self)
    }
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
        > + StateProviderFactory
        + CanonStateSubscriptions<Primitives = Evm::Primitives>
        + StageCheckpointReader
        + Send
        + Sync
        + Unpin
        + Clone
        + 'static,
    Evm: ConfigureEvm + Clone + 'static,
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
