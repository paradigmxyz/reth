use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{map::HashSet, Bytes, B256};
use alloy_rpc_types_eth::{state::StateOverride, BlockOverrides, Index};
use alloy_rpc_types_trace::{
    filter::TraceFilter,
    opcode::{BlockOpcodeGas, TransactionOpcodeGas},
    parity::*,
};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_eth_api::CallManyRequest;
use serde::{de, Deserialize, Deserializer};
use serde_json::{Map, Value};

/// Ethereum trace API
#[rpc(server, namespace = "trace")]
pub trait TraceApi<TxReq> {
    /// Executes the given call and returns a number of possible traces for it.
    #[method(name = "call")]
    async fn trace_call(
        &self,
        call: TxReq,
        trace_types: HashSet<TraceType>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<TraceResults>;

    /// Performs multiple call traces on top of the same block, defaulting to latest when no block
    /// is specified. Each call is executed with the preceding calls applied first, allowing
    /// dependent transactions to be traced.
    #[method(name = "callMany")]
    async fn trace_call_many(
        &self,
        calls: CallManyRequest<Vec<(TxReq, HashSet<TraceType>)>>,
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

    /// Replays a transaction, returning the traces or `None` if the transaction does not exist.
    #[method(name = "replayTransaction")]
    async fn replay_transaction(
        &self,
        transaction: B256,
        trace_types: HashSet<TraceType>,
    ) -> RpcResult<Option<TraceResultsWithTransactionHash>>;

    /// Returns traces created at given block.
    #[method(name = "block")]
    async fn trace_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>>;

    /// Returns traces matching given filter.
    ///
    /// This is similar to `eth_getLogs` but for traces. Omitted range bounds default to latest,
    /// and a bound may also be one of the `earliest`, `latest`, `safe` and `finalized` tags.
    #[method(name = "filter")]
    async fn trace_filter(
        &self,
        filter: TraceFilterRequest,
    ) -> RpcResult<Vec<LocalizedTransactionTrace>>;

    /// Returns the transaction trace at the given `traceAddress` path.
    ///
    /// An empty path selects the root, `[0]` selects its first child, and `[0, 1]` selects that
    /// child's second child. Returns `None` if the transaction or path does not exist.
    /// Callers requiring a flat index can index the result of `trace_transaction` instead.
    #[method(name = "get")]
    async fn trace_get(
        &self,
        hash: B256,
        indices: Vec<Index>,
    ) -> RpcResult<Option<LocalizedTransactionTrace>>;

    /// Returns all traces of given transaction.
    #[method(name = "transaction")]
    async fn trace_transaction(
        &self,
        hash: B256,
    ) -> RpcResult<Option<Vec<LocalizedTransactionTrace>>>;

    /// Returns all opcodes with their count and combined gas usage for the given transaction in no
    /// particular order.
    #[method(name = "transactionOpcodeGas")]
    async fn trace_transaction_opcode_gas(
        &self,
        tx_hash: B256,
    ) -> RpcResult<Option<TransactionOpcodeGas>>;

    /// Returns the opcodes of all transactions in the given block.
    ///
    /// This is the same as `trace_transactionOpcodeGas` but for all transactions in a block.
    #[method(name = "blockOpcodeGas")]
    async fn trace_block_opcode_gas(&self, block_id: BlockId) -> RpcResult<Option<BlockOpcodeGas>>;
}

/// The parameter of `trace_filter`: a [`TraceFilter`] whose `fromBlock` and `toBlock` may also be
/// a block tag.
///
/// [`TraceFilter`] types its bounds as block numbers, so a bound given as one of the tags
/// `earliest`, `latest`, `safe`, `finalized` or `pending` is taken out before the rest is parsed as
/// a [`TraceFilter`], and its `from_block` or `to_block` is then `None`. Numeric bounds are parsed
/// by [`TraceFilter`] as before. The server resolves the tags; it rejects `pending`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceFilterRequest {
    /// The filter, without the bounds given as tags.
    pub filter: TraceFilter,
    /// The `fromBlock` bound, when given as a tag.
    pub from_tag: Option<BlockNumberOrTag>,
    /// The `toBlock` bound, when given as a tag.
    pub to_tag: Option<BlockNumberOrTag>,
}

impl From<TraceFilter> for TraceFilterRequest {
    fn from(filter: TraceFilter) -> Self {
        Self { filter, from_tag: None, to_tag: None }
    }
}

impl<'de> Deserialize<'de> for TraceFilterRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut filter = Map::<String, Value>::deserialize(deserializer)?;
        let mut take_tag = |key: &str| {
            let tag = match filter.get(key)?.as_str()? {
                "earliest" => BlockNumberOrTag::Earliest,
                "latest" => BlockNumberOrTag::Latest,
                "safe" => BlockNumberOrTag::Safe,
                "finalized" => BlockNumberOrTag::Finalized,
                "pending" => BlockNumberOrTag::Pending,
                _ => return None,
            };
            filter.remove(key);
            Some(tag)
        };
        let from_tag = take_tag("fromBlock");
        let to_tag = take_tag("toBlock");
        let filter = TraceFilter::deserialize(Value::Object(filter)).map_err(de::Error::custom)?;
        Ok(Self { filter, from_tag, to_tag })
    }
}
