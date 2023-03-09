use crate::{eth::cache::EthStateCache, result::internal_rpc_err};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{BlockId, Bytes, H256};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_api::TraceApiServer;
use reth_rpc_types::{
    trace::{filter::TraceFilter, parity::*},
    CallRequest, Index,
};
use std::collections::HashSet;

/// `trace` API implementation.
///
/// This type provides the functionality for handling `trace` related requests.
#[derive(Clone)]
pub struct TraceApi<Client> {
    /// The client that can interact with the chain.
    client: Client,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
}

// === impl TraceApi ===

impl<Client> TraceApi<Client> {
    /// Create a new instance of the [TraceApi]
    pub fn new(client: Client, eth_cache: EthStateCache) -> Self {
        Self { client, eth_cache }
    }
}

#[async_trait]
impl<Client> TraceApiServer for TraceApi<Client>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    async fn call(
        &self,
        _call: CallRequest,
        _trace_types: HashSet<TraceType>,
        _block_id: Option<BlockId>,
    ) -> Result<TraceResults> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn call_many(
        &self,
        _calls: Vec<(CallRequest, HashSet<TraceType>)>,
        _block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn raw_transaction(
        &self,
        _data: Bytes,
        _trace_types: HashSet<TraceType>,
        _block_id: Option<BlockId>,
    ) -> Result<TraceResults> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn replay_block_transactions(
        &self,
        _block_id: BlockId,
        _trace_types: HashSet<TraceType>,
    ) -> Result<Option<Vec<TraceResultsWithTransactionHash>>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn replay_transaction(
        &self,
        _transaction: H256,
        _trace_types: HashSet<TraceType>,
    ) -> Result<TraceResults> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block(&self, _block_id: BlockId) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn filter(&self, _filter: TraceFilter) -> Result<Vec<LocalizedTransactionTrace>> {
        Err(internal_rpc_err("unimplemented"))
    }

    fn trace(
        &self,
        _hash: H256,
        _indices: Vec<Index>,
    ) -> Result<Option<LocalizedTransactionTrace>> {
        Err(internal_rpc_err("unimplemented"))
    }

    fn transaction_traces(&self, _hash: H256) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        Err(internal_rpc_err("unimplemented"))
    }
}

impl<Client> std::fmt::Debug for TraceApi<Client> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceApi").finish_non_exhaustive()
    }
}
