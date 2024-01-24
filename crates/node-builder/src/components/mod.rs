//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes].

pub use builder::*;
pub use network::*;
pub use payload::*;
pub use pool::*;
use reth_network::NetworkHandle;
use reth_node_api::node::FullNodeTypes;
use reth_node_core::node_config::NodeConfig;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::Head;
use reth_tasks::TaskExecutor;

pub use traits::*;

mod builder;
mod network;
mod payload;
mod pool;
mod traits;

/// Captures the necessary context for building the components of the node.
#[derive(Debug)]
pub struct BuilderContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    head: Head,
    /// The configured provider to interact with the blockchain.
    provider: Node::Provider,

    executor: TaskExecutor,

    // TODO maybe combine this with provider
    events: (),

    /// The data dir of the node.
    data_dir: (),
    /// The config of the node
    config: NodeConfig,
}

impl<Node: FullNodeTypes> BuilderContext<Node> {
    pub fn provider(&self) -> &Node::Provider {
        &self.provider
    }

    /// Returns the current head of the blockchain at launch.
    pub fn head(&self) -> Head {
        self.head
    }

    /// Returns the data dir of the node.
    pub fn data_dir(&self) -> &() {
        &self.data_dir
    }

    // TODO read only helper methods to access the config traits (cli args)
}

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct NodeComponents<Node: FullNodeTypes, Pool> {
    /// The transaction pool of the node.
    pub transaction_pool: Pool,
    /// The network implementation of the node.
    pub network: NetworkHandle,
    /// The handle to the payload builder service.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
}

impl<Node: FullNodeTypes, Pool> NodeComponents<Node, Pool> {
    /// Returns the handle to the payload builder service.
    pub fn payload_builder(&self) -> PayloadBuilderHandle<Node::Engine> {
        self.payload_builder.clone()
    }
}
