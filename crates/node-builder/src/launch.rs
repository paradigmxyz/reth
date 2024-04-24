//! Abstraction for launching a node.

use crate::NodeComponentsService;
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
pub struct DefaultLauncher {
    pub task_executor: TaskExecutor,
    pub data_dir: ChainPath<DataDirPath>,
}

// impl<T, DB, CB> LaunchNode<>  for DefaultLauncher
//     where
//         DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
//         T: NodeTypes,
//         CB: NodeComponentsService<FullNodeTypesAdapter<T, DB, RethFullProviderType<DB>>>,
//
// {
//
//
// }
