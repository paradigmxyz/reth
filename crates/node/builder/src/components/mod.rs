//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes](crate::node::FullNodeTypes).

mod builder;
mod consensus;
mod execute;
mod network;
mod payload;
mod pool;

pub use builder::*;
pub use consensus::*;
pub use execute::*;
pub use network::*;
pub use payload::*;
pub use pool::*;
use reth_network_p2p::BlockClient;
use reth_payload_builder::PayloadBuilderHandle;

use crate::{ConfigureEvm, FullNodeTypes};
use reth_consensus::{ConsensusError, FullConsensus};
use reth_evm::{execute::BlockExecutorProvider, ConfigureEvmFor};
use reth_network::{NetworkHandle, NetworkPrimitives};
use reth_network_api::FullNetwork;
use reth_node_api::{
    BlockTy, BodyTy, HeaderTy, NodeTypes, NodeTypesWithEngine, PayloadBuilderFor, PrimitivesTy,
    TxTy,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// An abstraction over the components of a node, consisting of:
///  - evm and executor
///  - transaction pool
///  - network
///  - payload builder.
pub trait NodeComponents<T: FullNodeTypes>: Clone + Unpin + Send + Sync + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<T::Types>>> + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvmFor<<T::Types as NodeTypes>::Primitives>;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider<Primitives = <T::Types as NodeTypes>::Primitives>;

    /// The consensus type of the node.
    type Consensus: FullConsensus<<T::Types as NodeTypes>::Primitives, Error = ConsensusError>
        + Clone
        + Unpin
        + 'static;

    /// Network API.
    type Network: FullNetwork<Client: BlockClient<Block = BlockTy<T::Types>>>;

    /// Builds new blocks.
    type PayloadBuilder: PayloadBuilderFor<T::Types> + Clone + Unpin + 'static;

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

    /// Returns the payload builder that knows how to build blocks.
    fn payload_builder(&self) -> &Self::PayloadBuilder;

    /// Returns the handle to the payload builder service handling payload building requests from
    /// the engine.
    fn payload_builder_handle(
        &self,
    ) -> &PayloadBuilderHandle<<T::Types as NodeTypesWithEngine>::Engine>;
}

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct Components<
    Node: FullNodeTypes,
    N: NetworkPrimitives,
    Pool,
    EVM,
    Executor,
    Consensus,
    Payload,
> {
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
    /// The payload builder.
    pub payload_builder: Payload,
    /// The handle to the payload builder service.
    pub payload_builder_handle: PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>,
}

impl<Node, Pool, EVM, Executor, Cons, N, Payload> NodeComponents<Node>
    for Components<Node, N, Pool, EVM, Executor, Cons, Payload>
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
    EVM: ConfigureEvm<Header = HeaderTy<Node::Types>, Transaction = TxTy<Node::Types>>,
    Executor: BlockExecutorProvider<Primitives = PrimitivesTy<Node::Types>>,
    Cons:
        FullConsensus<PrimitivesTy<Node::Types>, Error = ConsensusError> + Clone + Unpin + 'static,
    Payload: PayloadBuilderFor<Node::Types> + Clone + Unpin + 'static,
{
    type Pool = Pool;
    type Evm = EVM;
    type Executor = Executor;
    type Consensus = Cons;
    type Network = NetworkHandle<N>;
    type PayloadBuilder = Payload;

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

    fn payload_builder(&self) -> &Self::PayloadBuilder {
        &self.payload_builder
    }

    fn payload_builder_handle(
        &self,
    ) -> &PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine> {
        &self.payload_builder_handle
    }
}

impl<Node, N, Pool, EVM, Executor, Cons, Payload> Clone
    for Components<Node, N, Pool, EVM, Executor, Cons, Payload>
where
    N: NetworkPrimitives,
    Node: FullNodeTypes,
    Pool: TransactionPool,
    EVM: ConfigureEvm,
    Executor: BlockExecutorProvider,
    Cons: Clone,
    Payload: Clone,
{
    fn clone(&self) -> Self {
        Self {
            transaction_pool: self.transaction_pool.clone(),
            evm_config: self.evm_config.clone(),
            executor: self.executor.clone(),
            consensus: self.consensus.clone(),
            network: self.network.clone(),
            payload_builder: self.payload_builder.clone(),
            payload_builder_handle: self.payload_builder_handle.clone(),
        }
    }
}
