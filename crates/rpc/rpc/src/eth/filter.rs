use crate::result::{internal_rpc_err, ToRpcResult};
use async_trait::async_trait;
use jsonrpsee::{
    core::RpcResult,
    server::{IdProvider, RandomIntegerIdProvider},
    types::SubscriptionId,
};
use reth_primitives::rpc::Filter;
use reth_provider::BlockProvider;
use reth_rpc_api::EthFilterApiServer;
use reth_rpc_types::{FilterChanges, Log};
use reth_transaction_pool::TransactionPool;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::Mutex;
use tracing::trace;

/// `Eth` filter RPC implementation.
#[derive(Debug, Clone)]
pub struct EthFilter<Client, Pool> {
    /// All nested fields bundled together.
    inner: Arc<EthFilterInner<Client, Pool>>,
}

impl<Client, Pool> EthFilter<Client, Pool> {
    /// Creates a new, shareable instance.
    pub fn new(client: Client, pool: Pool) -> Self {
        let inner = EthFilterInner {
            client,
            active_filters: Default::default(),
            pool,
            id_provider: Arc::new(RandomIntegerIdProvider),
        };
        Self { inner: Arc::new(inner) }
    }

    /// Returns all currently active filters
    pub fn active_filters(&self) -> &ActiveFilters {
        &self.inner.active_filters
    }
}

#[async_trait]
impl<Client, Pool> EthFilterApiServer for EthFilter<Client, Pool>
where
    Client: BlockProvider + 'static,
    Pool: TransactionPool + 'static,
{
    async fn new_filter(&self, filter: Filter) -> RpcResult<SubscriptionId<'static>> {
        self.inner.install_filter(FilterKind::Log(filter)).await
    }

    async fn new_block_filter(&self) -> RpcResult<SubscriptionId<'static>> {
        self.inner.install_filter(FilterKind::Block).await
    }

    async fn new_pending_transaction_filter(&self) -> RpcResult<SubscriptionId<'static>> {
        self.inner.install_filter(FilterKind::PendingTransaction).await
    }

    async fn filter_changes(&self, _id: SubscriptionId<'_>) -> RpcResult<FilterChanges> {
        todo!()
    }

    async fn filter_logs(&self, _id: SubscriptionId<'_>) -> RpcResult<Vec<Log>> {
        todo!()
    }

    async fn uninstall_filter(&self, id: SubscriptionId<'_>) -> RpcResult<bool> {
        let mut filters = self.inner.active_filters.inner.lock().await;
        let id = id.into_owned();
        if filters.remove(&id).is_some() {
            trace!(target: "rpc::eth::filter", ?id, "uninstalled filter");
            Ok(true)
        } else {
            Err(internal_rpc_err(format!("Filter id {id:?} does not exist.")))
        }
    }

    async fn logs(&self, _filter: Filter) -> RpcResult<Vec<Log>> {
        todo!()
    }
}

/// Container type `EthFilter`
#[derive(Debug)]
struct EthFilterInner<Client, Pool> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Client,
    /// All currently installed filters.
    active_filters: ActiveFilters,
    id_provider: Arc<dyn IdProvider>,
}

impl<Client, Pool> EthFilterInner<Client, Pool>
where
    Client: BlockProvider + 'static,
    Pool: TransactionPool + 'static,
{
    /// Installs a new filter and returns the new identifier.
    async fn install_filter(&self, kind: FilterKind) -> RpcResult<SubscriptionId<'static>> {
        let last_poll_block_number = self.client.chain_info().to_rpc_result()?.best_number;
        let id = self.id_provider.next_id();
        let mut filters = self.active_filters.inner.lock().await;
        filters.insert(
            id.clone(),
            ActiveFilter { last_poll_block_number, last_poll_timestamp: Instant::now(), kind },
        );
        Ok(id)
    }
}

/// All active filters
#[derive(Debug, Clone, Default)]
pub struct ActiveFilters {
    inner: Arc<Mutex<HashMap<SubscriptionId<'static>, ActiveFilter>>>,
}

/// An installed filter
#[derive(Debug)]
struct ActiveFilter {
    /// At which block the filter was polled last.
    last_poll_block_number: u64,
    /// Last time this filter was polled.
    last_poll_timestamp: Instant,
    /// What kind of filter it is.
    kind: FilterKind,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // stored on heap
enum FilterKind {
    Log(Filter),
    Block,
    PendingTransaction,
}
