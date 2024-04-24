//! Abstraction for launching a node.

use crate::{
    builder::NodeAdapter, components::NodeComponentsBuilder, NodeBuilderWithComponents, NodeHandle,
    RethFullAdapter,
};
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_node_api::{FullNodeTypesAdapter, NodeTypes};
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_tasks::TaskExecutor;
use std::future::Future;

/// Launches a new node.
///
/// Acts as a node factory.
///
/// This is essentially the launch logic for a node.
pub trait LaunchNode<Target> {
    /// The node type that is created.
    type Node;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> + Send;
}

/// The default launcher for a node.
#[derive(Debug)]
pub struct DefaultNodeLauncher {
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The data directory for the node.
    pub data_dir: ChainPath<DataDirPath>,
}

impl<T, DB, CB> LaunchNode<NodeBuilderWithComponents<RethFullAdapter<DB, T>, CB>>
    for DefaultNodeLauncher
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
    T: NodeTypes,
    CB: NodeComponentsBuilder<RethFullAdapter<DB, T>>,
{
    type Node = NodeHandle<NodeAdapter<RethFullAdapter<DB, T>, CB::Components>>;

    async fn launch_node(
        self,
        target: NodeBuilderWithComponents<RethFullAdapter<DB, T>, CB>,
    ) -> eyre::Result<Self::Node> {
        todo!()
    }
}
