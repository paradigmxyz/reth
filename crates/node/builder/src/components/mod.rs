//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes](crate::node::FullNodeTypes).

use crate::{ConfigureEvm, FullNodeTypes};
pub use builder::*;
pub use execute::*;
pub use network::*;
pub use payload::*;
pub use pool::*;
use reth_network::NetworkHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::TransactionPool;

mod builder;
mod execute;
mod network;
mod payload;
mod pool;

/// An abstraction over the components of a node, consisting of:
///  - transaction pool
///  - network
///  - payload builder.
pub trait NodeComponents<NodeTypes: FullNodeTypes>: Clone + Send + Sync + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the handle to the network
    fn network(&self) -> &NetworkHandle;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<NodeTypes::Engine>;
}

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct Components<Node: FullNodeTypes, Pool, EVM> {
    /// The transaction pool of the node.
    pub transaction_pool: Pool,
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    pub evm_config: EVM,
    /// The network implementation of the node.
    pub network: NetworkHandle,
    /// The handle to the payload builder service.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
}

impl<Node, Pool, EVM> NodeComponents<Node> for Components<Node, Pool, EVM>
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
    EVM: ConfigureEvm,
{
    type Pool = Pool;
    type Evm = EVM;

    fn pool(&self) -> &Self::Pool {
        &self.transaction_pool
    }

    fn evm_config(&self) -> &Self::Evm {
        &self.evm_config
    }

    fn network(&self) -> &NetworkHandle {
        &self.network
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<Node::Engine> {
        &self.payload_builder
    }
}

impl<Node, Pool, EVM> Clone for Components<Node, Pool, EVM>
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    EVM: ConfigureEvm,
{
    fn clone(&self) -> Self {
        Self {
            transaction_pool: self.transaction_pool.clone(),
            evm_config: self.evm_config.clone(),
            network: self.network.clone(),
            payload_builder: self.payload_builder.clone(),
        }
    }
}
