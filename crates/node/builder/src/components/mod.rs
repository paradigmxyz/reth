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
mod engine;
mod execute;
mod network;
mod payload;
mod pool;

pub use builder::*;
pub use consensus::*;
pub use engine::*;
pub use execute::*;
pub use network::*;
pub use payload::*;
pub use pool::*;

use reth_consensus::Consensus;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::NetworkHandle;
use reth_network_api::FullNetwork;
use reth_node_api::{EngineValidator, NodeTypesWithEngine};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::Header;
use reth_transaction_pool::TransactionPool;

use crate::{ConfigureEvm, FullNodeTypes};

/// An abstraction over the components of a node, consisting of:
///  - evm and executor
///  - transaction pool
///  - network
///  - payload builder.
pub trait NodeComponents<T: FullNodeTypes>: Clone + Unpin + Send + Sync + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Header = Header>;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider;

    /// The consensus type of the node.
    type Consensus: Consensus + Clone + Unpin + 'static;

    /// Network API.
    type Network: FullNetwork;

    /// Validator for the engine API.
    type EngineValidator: EngineValidator<<T::Types as NodeTypesWithEngine>::Engine>;

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

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<<T::Types as NodeTypesWithEngine>::Engine>;

    /// Returns the engine validator.
    fn engine_validator(&self) -> &Self::EngineValidator;
}

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct Components<Node: FullNodeTypes, Pool, EVM, Executor, Consensus, Validator> {
    /// The transaction pool of the node.
    pub transaction_pool: Pool,
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    pub evm_config: EVM,
    /// The node's executor type used to execute individual blocks and batches of blocks.
    pub executor: Executor,
    /// The consensus implementation of the node.
    pub consensus: Consensus,
    /// The network implementation of the node.
    pub network: NetworkHandle,
    /// The handle to the payload builder service.
    pub payload_builder: PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>,
    /// The validator for the engine API.
    pub engine_validator: Validator,
}

impl<Node, Pool, EVM, Executor, Cons, Val> NodeComponents<Node>
    for Components<Node, Pool, EVM, Executor, Cons, Val>
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
    EVM: ConfigureEvm<Header = Header>,
    Executor: BlockExecutorProvider,
    Cons: Consensus + Clone + Unpin + 'static,
    Val: EngineValidator<<Node::Types as NodeTypesWithEngine>::Engine> + Clone + Unpin + 'static,
{
    type Pool = Pool;
    type Evm = EVM;
    type Executor = Executor;
    type Consensus = Cons;
    type Network = NetworkHandle;
    type EngineValidator = Val;

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

    fn payload_builder(
        &self,
    ) -> &PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine> {
        &self.payload_builder
    }

    fn engine_validator(&self) -> &Self::EngineValidator {
        &self.engine_validator
    }
}

impl<Node, Pool, EVM, Executor, Cons, Val> Clone
    for Components<Node, Pool, EVM, Executor, Cons, Val>
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    EVM: ConfigureEvm<Header = Header>,
    Executor: BlockExecutorProvider,
    Cons: Consensus + Clone,
    Val: EngineValidator<<Node::Types as NodeTypesWithEngine>::Engine>,
{
    fn clone(&self) -> Self {
        Self {
            transaction_pool: self.transaction_pool.clone(),
            evm_config: self.evm_config.clone(),
            executor: self.executor.clone(),
            consensus: self.consensus.clone(),
            network: self.network.clone(),
            payload_builder: self.payload_builder.clone(),
            engine_validator: self.engine_validator.clone(),
        }
    }
}
