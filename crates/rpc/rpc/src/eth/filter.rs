use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{rpc::Filter, U256};
use reth_provider::BlockProvider;
use reth_rpc_api::EthFilterApiServer;
use reth_rpc_types::{FilterChanges, Index, Log};
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

/// `Eth` filter RPC implementation.
#[derive(Debug, Clone)]
pub struct EthFilter<Client, Pool> {
    /// All nested fields bundled together.
    inner: Arc<EthFilterInner<Client, Pool>>,
}

impl<Client, Pool> EthFilter<Client, Pool> {
    /// Creates a new, shareable instance.
    pub fn new(client: Arc<Client>, pool: Pool) -> Self {
        let inner = EthFilterInner { client, pool };
        Self { inner: Arc::new(inner) }
    }
}

#[async_trait]
impl<Client, Pool> EthFilterApiServer for EthFilter<Client, Pool>
where
    Client: BlockProvider + 'static,
    Pool: TransactionPool + 'static,
{
    fn new_filter(&self, _filter: Filter) -> RpcResult<U256> {
        todo!()
    }

    fn new_block_filter(&self) -> RpcResult<U256> {
        todo!()
    }

    fn new_pending_transaction_filter(&self) -> RpcResult<U256> {
        todo!()
    }

    async fn filter_changes(&self, _index: Index) -> RpcResult<FilterChanges> {
        todo!()
    }

    async fn filter_logs(&self, _index: Index) -> RpcResult<Vec<Log>> {
        todo!()
    }

    fn uninstall_filter(&self, _index: Index) -> RpcResult<bool> {
        todo!()
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
    client: Arc<Client>,
    // TODO needs spawn access
}
