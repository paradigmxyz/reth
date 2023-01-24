use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{rpc::BlockId, Bytes, H256};
use reth_rpc_api::TraceApiServer;
use reth_rpc_types::{
    trace::{filter::TraceFilter, parity::*},
    CallRequest, Index,
};
use std::collections::HashSet;

/// `trace` API implementation.
///
/// This type provides the functionality for handling `trace` related requests.
#[non_exhaustive]
pub struct TraceApi {}

#[async_trait]
impl TraceApiServer for TraceApi {
    async fn call(
        &self,
        _call: CallRequest,
        _trace_types: HashSet<TraceType>,
        _block_id: Option<BlockId>,
    ) -> Result<TraceResults> {
        todo!()
    }

    async fn call_many(
        &self,
        _calls: Vec<(CallRequest, HashSet<TraceType>)>,
        _block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>> {
        todo!()
    }

    async fn raw_transaction(
        &self,
        _data: Bytes,
        _trace_types: HashSet<TraceType>,
        _block_id: Option<BlockId>,
    ) -> Result<TraceResults> {
        todo!()
    }

    async fn replay_block_transactions(
        &self,
        _block_id: BlockId,
        _trace_types: HashSet<TraceType>,
    ) -> Result<Option<Vec<TraceResultsWithTransactionHash>>> {
        todo!()
    }

    async fn replay_transaction(
        &self,
        _transaction: H256,
        _trace_types: HashSet<TraceType>,
    ) -> Result<TraceResults> {
        todo!()
    }

    async fn block(&self, _block_id: BlockId) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        todo!()
    }

    async fn filter(&self, _filter: TraceFilter) -> Result<Vec<LocalizedTransactionTrace>> {
        todo!()
    }

    fn trace(
        &self,
        _hash: H256,
        _indices: Vec<Index>,
    ) -> Result<Option<LocalizedTransactionTrace>> {
        todo!()
    }

    fn transaction_traces(&self, _hash: H256) -> Result<Option<Vec<LocalizedTransactionTrace>>> {
        todo!()
    }
}

impl std::fmt::Debug for TraceApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceApi").finish_non_exhaustive()
    }
}
