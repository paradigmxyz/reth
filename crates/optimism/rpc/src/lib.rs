//! Standalone crate for Optimism-specific RPC types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::sync::Arc;

use reth_rpc::eth::api::{EthApiInner, SpawnBlocking};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner};

pub mod block;
pub mod error;
pub mod receipt;
pub mod transaction;

/// `Eth` API implementation for OP networks.
///
/// This type provides OP specific extension of default functionality for handling `eth_` related
/// requests. See [`EthApi`](reth_rpc::EthApi) for the default L1 implementation.
#[allow(missing_debug_implementations)]
#[allow(dead_code)]
#[derive(Clone)]
pub struct OptimismApi<Provider, Pool, Network, EvmConfig> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Provider, Pool, Network, EvmConfig>>,
}

impl<Provider, Pool, Network, EvmConfig> SpawnBlocking
    for OptimismApi<Provider, Pool, Network, EvmConfig>
where
    Self: Clone + Send + Sync + 'static,
{
    fn io_task_spawner(&self) -> impl TaskSpawner {
        self.inner.task_spawner()
    }

    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.blocking_task_pool()
    }
}
