use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{BlockId, Bytes, H256};
use reth_rpc_types::{
    trace::{filter::TraceFilter, parity::*},
    CallRequest, Index,
};
use std::collections::HashSet;

/// Ethereum trace API
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait TraceApi {
    /// Executes the given call and returns a number of possible traces for it.
    #[method(name = "trace_call")]
    async fn call(
        &self,
        call: CallRequest,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> Result<TraceResults>;

    /// Performs multiple call traces on top of the same block. i.e. transaction n will be executed
    /// on top of a pending block with all n-1 transactions applied (traced) first. Allows to trace
    /// dependent transactions.
    #[method(name = "trace_callMany")]
    async fn call_many(
        &self,
        calls: Vec<(CallRequest, HashSet<TraceType>)>,
        block_id: Option<BlockId>,
    ) -> Result<Vec<TraceResults>>;

    /// Traces a call to `eth_sendRawTransaction` without making the call, returning the traces.
    ///
    /// Expects a raw transaction data
    #[method(name = "trace_rawTransaction")]
    async fn raw_transaction(
        &self,
        data: Bytes,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
    ) -> Result<TraceResults>;

    /// Replays all transactions in a block returning the requested traces for each transaction.
    #[method(name = "trace_replayBlockTransactions")]
    async fn replay_block_transactions(
        &self,
        block_id: BlockId,
        trace_types: HashSet<TraceType>,
    ) -> Result<Option<Vec<TraceResultsWithTransactionHash>>>;

    /// Replays a transaction, returning the traces.
    #[method(name = "trace_replayTransaction")]
    async fn replay_transaction(
        &self,
        transaction: H256,
        trace_types: HashSet<TraceType>,
    ) -> Result<TraceResults>;

    /// Returns traces created at given block.
    #[method(name = "trace_block")]
    async fn block(&self, block_id: BlockId) -> Result<Option<Vec<LocalizedTransactionTrace>>>;

    /// Returns traces matching given filter
    #[method(name = "trace_filter")]
    async fn filter(&self, filter: TraceFilter) -> Result<Vec<LocalizedTransactionTrace>>;

    /// Returns transaction trace at given index.
    #[method(name = "trace_get")]
    fn trace(&self, hash: H256, indices: Vec<Index>) -> Result<Option<LocalizedTransactionTrace>>;

    /// Returns all traces of given transaction.
    #[method(name = "trace_transaction")]
    fn transaction_traces(&self, hash: H256) -> Result<Option<Vec<LocalizedTransactionTrace>>>;
}
