//! Helper trait for interfacing with [`FullNodeComponents`].

use reth_chainspec::ChainSpecProvider;
use reth_evm::ConfigureEvm;
use reth_node_api::{FullNodeComponents, NodePrimitives, PrimitivesTy};
use reth_primitives_traits::{BlockTy, HeaderTy, ReceiptTy, TxTy};
use reth_rpc_eth_types::EthStateCache;
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, NodePrimitivesProvider, StateProviderFactory,
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
pub trait RpcNodeCore: Clone + Send + Sync {
    /// Blockchain data primitives.
    type Primitives: NodePrimitives;
    /// The provider type used to interact with the node.
    type Provider: BlockReaderIdExt<
            Block = BlockTy<Self::Primitives>,
            Receipt = ReceiptTy<Self::Primitives>,
            Header = HeaderTy<Self::Primitives>,
            Transaction = TxTy<Self::Primitives>,
        > + NodePrimitivesProvider<Primitives = Self::Primitives>
        + ChainSpecProvider
        + StateProviderFactory
        + Send
        + Sync
        + 'static;
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Self::Primitives>>>;
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Primitives = Self::Primitives> + Send + Sync + 'static;
    /// Network API.
    type Network: Send + Sync + Clone;

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
    T: FullNodeComponents,
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
