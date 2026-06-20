//! Reth-specific engine API extensions.

use alloy_primitives::Bytes;
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdated, PayloadStatus};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use serde::{ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};

/// Reth-specific payload status that includes server-measured execution latency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RethPayloadStatus {
    /// The standard payload status.
    #[serde(flatten)]
    pub status: PayloadStatus,
    /// Server-side execution latency in microseconds.
    pub latency_us: u64,
    /// Time spent waiting on persistence in microseconds, including both time spent
    /// queued due to persistence backpressure and, when requested, the explicit wait
    /// for in-flight persistence to complete.
    pub persistence_wait_us: u64,
    /// Time spent waiting for the execution cache lock, in microseconds.
    ///
    /// `None` when wasn't asked to wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_cache_wait_us: Option<u64>,
    /// Time spent waiting for the sparse trie lock, in microseconds.
    ///
    /// `None` when wasn't asked to wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparse_trie_wait_us: Option<u64>,
}

/// Input for `reth_newPayload` that accepts either `ExecutionData` directly or an RLP-encoded
/// block with optional side data.
#[derive(Debug, Clone)]
pub enum RethNewPayloadInput<ExecutionData> {
    /// Standard execution data (payload + sidecar).
    ExecutionData(ExecutionData),
    /// An RLP-encoded block and optional encoded block access list.
    BlockRlp {
        /// RLP-encoded block bytes.
        block: Bytes,
        /// RLP-encoded block access list bytes.
        bal: Option<Bytes>,
    },
}

impl<ExecutionData> Serialize for RethNewPayloadInput<ExecutionData>
where
    ExecutionData: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::ExecutionData(data) => data.serialize(serializer),
            Self::BlockRlp { block, bal: None } => block.serialize(serializer),
            Self::BlockRlp { block, bal: Some(bal) } => {
                let mut state = serializer.serialize_struct("BlockRlp", 2)?;
                state.serialize_field("block", block)?;
                state.serialize_field("bal", bal)?;
                state.end()
            }
        }
    }
}

impl<'de, ExecutionData> Deserialize<'de> for RethNewPayloadInput<ExecutionData>
where
    ExecutionData: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RethNewPayloadInputSerde<ExecutionData> {
            ExecutionData(ExecutionData),
            BlockRlp {
                block: Bytes,
                #[serde(default)]
                bal: Option<Bytes>,
            },
            LegacyBlockRlp(Bytes),
        }

        Ok(match RethNewPayloadInputSerde::deserialize(deserializer)? {
            RethNewPayloadInputSerde::ExecutionData(data) => Self::ExecutionData(data),
            RethNewPayloadInputSerde::BlockRlp { block, bal } => Self::BlockRlp { block, bal },
            RethNewPayloadInputSerde::LegacyBlockRlp(block) => Self::BlockRlp { block, bal: None },
        })
    }
}

/// Reth-specific engine API extensions.
///
/// This trait provides a `reth_newPayload` endpoint that accepts either `ExecutionData` directly
/// (payload + sidecar) or an RLP-encoded block, optionally alongside a block access list and
/// waiting for persistence and cache locks before processing.
///
/// By default, the endpoint waits for cache updates but lets in-flight persistence continue in the
/// background before executing the payload. Persistence waiting can be explicitly enabled with
/// `wait_for_persistence`, and cache waiting can be disabled with `wait_for_caches`.
///
/// Responses include timing breakdowns with server-measured execution latency.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "reth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "reth"))]
pub trait RethEngineApi<ExecutionData> {
    /// Reth-specific newPayload that accepts either `ExecutionData` directly or an RLP-encoded
    /// block.
    ///
    /// `wait_for_persistence` (default `false`): waits for in-flight persistence to complete.
    /// `wait_for_caches` (default `true`): waits for execution cache and sparse trie locks.
    #[method(name = "newPayload")]
    async fn reth_new_payload(
        &self,
        payload: RethNewPayloadInput<ExecutionData>,
        wait_for_persistence: Option<bool>,
        wait_for_caches: Option<bool>,
    ) -> RpcResult<RethPayloadStatus>;

    /// Reth-specific forkchoiceUpdated that sends a regular forkchoice update with no payload
    /// attributes.
    #[method(name = "forkchoiceUpdated")]
    async fn reth_forkchoice_updated(
        &self,
        forkchoice_state: ForkchoiceState,
    ) -> RpcResult<ForkchoiceUpdated>;
}

#[cfg(test)]
mod tests {
    use super::RethNewPayloadInput;
    use alloy_primitives::Bytes;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestExecutionData {
        payload: Bytes,
        sidecar: Bytes,
    }

    #[test]
    fn block_rlp_serializes_named_fields() {
        let input = RethNewPayloadInput::<TestExecutionData>::BlockRlp {
            block: Bytes::from_static(&[1]),
            bal: Some(Bytes::from_static(&[2])),
        };

        assert_eq!(serde_json::to_string(&input).unwrap(), r#"{"block":"0x01","bal":"0x02"}"#);
        assert_eq!(serde_json::to_value(input).unwrap(), json!({ "block": "0x01", "bal": "0x02" }));
    }

    #[test]
    fn block_rlp_without_bal_serializes_legacy_bytes_roundtrip() {
        let input = RethNewPayloadInput::<TestExecutionData>::BlockRlp {
            block: Bytes::from_static(&[1]),
            bal: None,
        };

        let serialized = serde_json::to_string(&input).unwrap();
        assert_eq!(serialized, r#""0x01""#);

        let input =
            serde_json::from_str::<RethNewPayloadInput<TestExecutionData>>(&serialized).unwrap();

        let RethNewPayloadInput::BlockRlp { block, bal } = input else {
            panic!("expected block rlp input")
        };

        assert_eq!(block, Bytes::from_static(&[1]));
        assert_eq!(bal, None);
    }

    #[test]
    fn block_rlp_deserializes_without_bal() {
        let input =
            serde_json::from_str::<RethNewPayloadInput<TestExecutionData>>(r#"{"block":"0x01"}"#)
                .unwrap();

        let RethNewPayloadInput::BlockRlp { block, bal } = input else {
            panic!("expected block rlp input")
        };

        assert_eq!(block, Bytes::from_static(&[1]));
        assert_eq!(bal, None);
    }

    #[test]
    fn block_rlp_deserializes_legacy_bytes() {
        let input = serde_json::from_value::<RethNewPayloadInput<TestExecutionData>>(json!("0x01"))
            .unwrap();

        let RethNewPayloadInput::BlockRlp { block, bal } = input else {
            panic!("expected block rlp input")
        };

        assert_eq!(block, Bytes::from_static(&[1]));
        assert_eq!(bal, None);
    }
}
