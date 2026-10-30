use alloy_eips::BlockId;
use alloy_primitives::{map::HashSet, Bytes, B256};
use alloy_rpc_types_eth::{state::StateOverride, BlockOverrides};
use alloy_rpc_types_trace::{
    filter::TraceFilter,
    opcode::{BlockOpcodeGas, TransactionOpcodeGas},
    parity::*,
};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Ethereum trace API
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "trace"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "trace"))]
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
        calls: Vec<(TxReq, HashSet<TraceType>)>,
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
    /// This is similar to `eth_getLogs` but for traces. Omitted range bounds default to latest.
    #[method(name = "filter")]
    async fn trace_filter(&self, filter: TraceFilter) -> RpcResult<Vec<LocalizedTransactionTrace>>;

    /// Returns the transaction trace at the given `traceAddress` path.
    ///
    /// An empty path selects the root, `["0x0"]` selects its first child, and `["0x0", "0x1"]`
    /// selects that child's second child. Path entries are hex quantities. Returns `None` if the
    /// transaction or path does not exist.
    /// Callers requiring a flat index can index the result of `trace_transaction` instead.
    #[method(name = "get")]
    async fn trace_get(
        &self,
        hash: B256,
        indices: Vec<TraceAddressIndex>,
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

/// An index into a `traceAddress` path, encoded as a hex quantity such as `"0x1"`.
///
/// Unlike [`alloy_rpc_types_eth::Index`], JSON numbers, decimal strings and hex strings with
/// leading zeros or uppercase digits are rejected, so `traceAddress` entries taken from a trace
/// must be converted to hex quantities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceAddressIndex(pub usize);

impl From<TraceAddressIndex> for usize {
    fn from(index: TraceAddressIndex) -> Self {
        index.0
    }
}

impl Serialize for TraceAddressIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{:#x}", self.0))
    }
}

impl<'de> Deserialize<'de> for TraceAddressIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let digits = value
            .strip_prefix("0x")
            .filter(|digits| {
                matches!(digits.as_bytes(), [b'0'] | [b'1'..=b'9' | b'a'..=b'f', ..]) &&
                    digits.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            })
            .ok_or_else(|| {
                de::Error::invalid_value(de::Unexpected::Str(&value), &"a hex quantity")
            })?;
        usize::from_str_radix(digits, 16).map(Self).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trace_address_index_accepts_hex_quantities() {
        for (value, expected) in [("0x0", 0), ("0x6", 6), ("0xa0", 160)] {
            let index = serde_json::from_value::<TraceAddressIndex>(json!(value)).unwrap();
            assert_eq!(index, TraceAddressIndex(expected));
            assert_eq!(serde_json::to_value(index).unwrap(), json!(value));
        }
        let max = format!("{:#x}", usize::MAX);
        let index = serde_json::from_value::<TraceAddressIndex>(json!(max)).unwrap();
        assert_eq!(index, TraceAddressIndex(usize::MAX));
        assert_eq!(serde_json::to_value(index).unwrap(), json!(max));
    }

    #[test]
    fn trace_address_index_rejects_other_encodings() {
        assert_eq!(
            serde_json::from_value::<TraceAddressIndex>(json!(6)).unwrap_err().to_string(),
            "invalid type: integer `6`, expected a string"
        );
        for value in ["6", "0x", "0x00", "0x06", "0xA", "0x1A", "0x+1"] {
            assert_eq!(
                serde_json::from_value::<TraceAddressIndex>(json!(value)).unwrap_err().to_string(),
                format!("invalid value: string {value:?}, expected a hex quantity")
            );
        }
        let overflow = format!("{:#x}0", usize::MAX);
        assert_eq!(
            serde_json::from_value::<TraceAddressIndex>(json!(overflow)).unwrap_err().to_string(),
            "number too large to fit in target type"
        );
    }
}
