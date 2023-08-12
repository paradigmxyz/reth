use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{BlockId, Bytes, H256};
use reth_rpc_types::{
    state::StateOverride,
    trace::{filter::TraceFilter, parity::*},
    BlockOverrides, CallRequest, Index,
};
use std::collections::HashSet;

/// Ethereum trace API
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "trace"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "trace"))]
pub trait TraceApi {
    /// Executes the given call and returns a number of possible traces for it.
    #[method(name = "call")]
    async fn trace_call(
        &self,
        call: CallRequest,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<TraceResults>;

    /// Performs multiple call traces on top of the same block. i.e. transaction n will be executed
    /// on top of a pending block with all n-1 transactions applied (traced) first. Allows to trace
    /// dependent transactions.
    #[method(name = "callMany")]
    async fn trace_call_many(
        &self,
        calls: Vec<(CallRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> RpcResult<Vec<TraceResults>>;

    /// Traces a call to `eth_sendRawTransaction` without making the call, returning the traces.
    ///
    /// Expects a raw transaction data
    #[method(name = "rawTransaction")]
    async fn trace_raw_transaction(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> RpcResult<TraceResults>;

    /// Replays all transactions in a block returning the requested traces for each transaction.
    #[method(name = "replayBlockTransactions")]
    async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<Option<Vec<TraceResultsWithTransactionHash>>>;

    /// Replays a transaction, returning the traces.
    #[method(name = "replayTransaction")]
    async fn replay_transaction(
        &self,
        transaction: H256,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<TraceResults>;

    /// Returns traces created at given block.
    #[method(name = "block")]
    async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>>;

    /// Returns traces matching given filter
    #[method(name = "filter")]
    async fn trace_filter(&self, filter: TraceFilter) -> RpcResult<Vec<LocalizedTransactionTrace>>;

    /// Returns transaction trace at given index.
    ///
    /// `indices` represent the index positions of the traces.
    ///
    /// Note: This expects a list of indices but only one is supported since this function returns a
    /// single [LocalizedTransactionTrace].
    #[method(name = "get")]
    async fn trace_get(
        &self,
        hash: H256,
        indices: Vec<Index>,
    ) -> RpcResult<Option<LocalizedTransactionTrace>>;

    /// Returns all traces of given transaction.
    #[method(name = "transaction")]
    async fn trace_transaction(
        &self,
        hash: H256,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>>;
}
