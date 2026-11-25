//! Bounded RPC input for `eth_simulateV1`.

use crate::call_many::invalid_params;
use alloy_rpc_types_eth::simulate::{
    SimulatePayload, SimulatePayloadLimitExceeded, SimulatePayloadSeed,
};
use reth_rpc_eth_types::{
    block_overrides::deserialize_block_override_keys, simulate::EthSimulateError, EthApiError,
};
use serde::{
    de::{DeserializeOwned, DeserializeSeed, IgnoredAny},
    Deserialize, Deserializer, Serialize,
};
use serde_json::value::RawValue;

/// Maximum number of calls accepted in one `eth_simulateV1` request.
const MAX_SIMULATE_CALLS: usize = 4096;

/// An `eth_simulateV1` payload that is decoded after the configured block limit is known.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SimulateV1Request<TxReq> {
    /// Raw RPC input, retained until the handler can apply its configured limits.
    Raw(Box<RawValue>),
    /// A typed payload supplied by an RPC client or internal caller.
    Parsed(SimulatePayload<TxReq>),
}

impl<TxReq> From<SimulatePayload<TxReq>> for SimulateV1Request<TxReq> {
    fn from(payload: SimulatePayload<TxReq>) -> Self {
        Self::Parsed(payload)
    }
}

impl<'de, TxReq> Deserialize<'de> for SimulateV1Request<TxReq> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Box::<RawValue>::deserialize(deserializer).map(Self::Raw)
    }
}

impl<TxReq: DeserializeOwned> SimulateV1Request<TxReq> {
    /// Decodes RPC input with the configured block limit and the call limit.
    pub fn into_inner(self, max_blocks: usize) -> Result<SimulatePayload<TxReq>, EthApiError> {
        let raw = match self {
            Self::Raw(raw) => raw,
            Self::Parsed(payload) => return Ok(payload),
        };

        let mut deserializer = serde_json::Deserializer::from_str(raw.get());
        let payload = SimulatePayloadSeed::<TxReq>::new(max_blocks, MAX_SIMULATE_CALLS)
            .deserialize(&mut deserializer)
            .map_err(|err| EthApiError::InvalidParams(err.to_string()))?;
        deserializer.end().map_err(|err| EthApiError::InvalidParams(err.to_string()))?;
        let payload = match payload {
            Ok(payload) => payload,
            Err(SimulatePayloadLimitExceeded::Blocks) => {
                return Err(EthApiError::other(EthSimulateError::TooManyBlocks))
            }
            Err(err) => return Err(EthApiError::InvalidParams(err.to_string())),
        };

        // alloy decodes `blockOverrides` to `None` only when it is missing or `null`, which have no
        // keys to check.
        if payload.block_state_calls.iter().any(|block| block.block_overrides.is_some()) {
            // The bounded decode succeeded, so this pass only walks input within the limits.
            serde_json::from_str::<PayloadOverrideKeys>(raw.get()).map_err(invalid_params)?;
        }
        Ok(payload)
    }
}

/// The `blockOverrides` keys of an `eth_simulateV1` payload, checked while decoding.
///
/// alloy's [`BlockOverrides`](alloy_rpc_types_eth::BlockOverrides) drops unknown keys, so they
/// are checked on the raw input. Fields mirror [`SimulatePayload`] and
/// [`SimBlock`](alloy_rpc_types_eth::simulate::SimBlock) in order, so the positional array forms
/// decode too.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(dead_code, reason = "fields are only decoded")]
struct PayloadOverrideKeys {
    /// Zero-sized elements, so this does not allocate.
    #[serde(default)]
    block_state_calls: Vec<BlockOverrideKeys>,
    #[serde(default)]
    trace_transfers: IgnoredAny,
    #[serde(default)]
    validation: IgnoredAny,
    #[serde(default)]
    return_full_transactions: IgnoredAny,
}

/// The `blockOverrides` keys of one `eth_simulateV1` block.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(dead_code, reason = "fields are only decoded")]
struct BlockOverrideKeys {
    #[serde(default, deserialize_with = "deserialize_block_override_keys")]
    block_overrides: (),
    #[serde(default)]
    state_overrides: IgnoredAny,
    #[serde(default)]
    calls: IgnoredAny,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rpc_types_eth::{simulate::SimBlock, TransactionRequest};
    use jsonrpsee_types::error::ErrorObjectOwned;

    fn payload_with_calls(counts: &[usize]) -> String {
        let blocks = counts
            .iter()
            .map(|&count| format!(r#"{{"calls":[{}]}}"#, vec!["{}"; count].join(",")))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"blockStateCalls":[{blocks}]}}"#)
    }

    fn parse(
        input: &str,
        max_blocks: usize,
    ) -> Result<SimulatePayload<TransactionRequest>, EthApiError> {
        serde_json::from_str::<SimulateV1Request<TransactionRequest>>(input)
            .unwrap()
            .into_inner(max_blocks)
    }

    #[test]
    fn uses_configured_block_limit() {
        let input = payload_with_calls(&vec![0; 1025]);
        assert_eq!(parse(&input, 1025).unwrap().block_state_calls.len(), 1025);
        let err: ErrorObjectOwned = parse(&input, 1024).unwrap_err().into();
        assert_eq!(err.code(), -38026);
    }

    #[test]
    fn bounds_calls_across_blocks() {
        let input = payload_with_calls(&[MAX_SIMULATE_CALLS - 1, 2]);
        let err: ErrorObjectOwned = parse(&input, 2).unwrap_err().into();
        assert_eq!(err.code(), -32602);
    }

    #[test]
    fn accepts_call_limit() {
        let input = payload_with_calls(&[MAX_SIMULATE_CALLS]);
        assert_eq!(parse(&input, 1).unwrap().block_state_calls[0].calls.len(), MAX_SIMULATE_CALLS);
    }

    #[test]
    fn preserves_payload_fields() {
        let input = r#"{"blockStateCalls":[{"calls":[{}]}],"traceTransfers":true,"validation":true,"returnFullTransactions":true}"#;
        let expected = serde_json::from_str::<SimulatePayload<TransactionRequest>>(input).unwrap();
        let parsed = parse(input, 1).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), serde_json::to_value(expected).unwrap());
    }

    fn payload_with_overrides(overrides: &[&str]) -> String {
        let blocks = overrides
            .iter()
            .map(|overrides| format!(r#"{{"blockOverrides":{overrides},"calls":[{{}}]}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"blockStateCalls":[{blocks}]}}"#)
    }

    fn assert_unsupported_key(input: &str, max_blocks: usize, key: &str) {
        let err = parse(input, max_blocks).unwrap_err();
        assert!(
            matches!(&err, EthApiError::InvalidParams(msg)
                if msg.starts_with(&format!("unsupported block override key: {key} "))),
            "{err:?}"
        );
        let err: ErrorObjectOwned = err.into();
        assert_eq!(err.code(), -32602);
    }

    #[test]
    fn rejects_unsupported_block_override_keys() {
        for (overrides, key) in [
            (r#"{"fooBar":"0x1"}"#, "fooBar"),
            (r#"{"Time":"0x1"}"#, "Time"),
            (r#"{"time":"0x1","Time":"0x1"}"#, "Time"),
            (r#"{"Time":"0x1","time":"0x1"}"#, "Time"),
            (r#"{"\u0074ime":"0x1","fo\u006f":"0x1"}"#, "foo"),
        ] {
            assert_unsupported_key(&payload_with_overrides(&[overrides]), 1, key);
        }
    }

    #[test]
    fn rejects_unsupported_key_in_later_block() {
        let input = payload_with_overrides(&[r#"{"time":"0x1"}"#, "null", r#"{"fooBar":"0x1"}"#]);
        assert_unsupported_key(&input, 3, "fooBar");
    }

    #[test]
    fn accepts_supported_block_override_keys() {
        let hash = format!("0x{}", "11".repeat(32));
        let address = format!("0x{}", "11".repeat(20));
        let overrides = [
            r#"{"number":"0x1","time":"0x2","baseFee":"0x3","random":"HASH","coinbase":"ADDR"}"#
                .to_string(),
            r#"{"blockNumber":"0x1","timestamp":"0x2","feeRecipient":"ADDR","prevRandao":"HASH","baseFeePerGas":"0x3","parentBeaconBlockRoot":"HASH"}"#
                .to_string(),
            r#"{"difficulty":"0x1","gasLimit":"0x2","blobBaseFee":"0x3","beaconRoot":"HASH","withdrawals":[],"blockHash":{"1":"HASH"}}"#
                .to_string(),
            "{}".to_string(),
            "null".to_string(),
            r#"["0x1"]"#.to_string(),
        ]
        .map(|overrides| overrides.replace("HASH", &hash).replace("ADDR", &address));
        let input = payload_with_overrides(&overrides.each_ref().map(String::as_str));
        let blocks = parse(&input, overrides.len()).unwrap().block_state_calls;
        assert_eq!(blocks.len(), overrides.len());
        assert!(blocks[0].block_overrides.as_ref().unwrap().time.is_some());
        assert!(blocks[4].block_overrides.is_none());
        assert_eq!(
            parse(r#"{"blockStateCalls":[{"calls":[{}]}]}"#, 1).unwrap().block_state_calls.len(),
            1
        );
    }

    #[test]
    fn checks_keys_in_positional_payload() {
        let input = r#"[[[{"time":"0x1"},null,[{}]]],true]"#;
        assert_eq!(parse(input, 1).unwrap().block_state_calls[0].calls.len(), 1);
        assert_unsupported_key(r#"[[{"blockOverrides":{"fooBar":"0x1"}}]]"#, 1, "fooBar");
    }

    #[test]
    fn block_limit_precedes_unsupported_keys() {
        let input = payload_with_overrides(&[r#"{"fooBar":"0x1"}"#; 2]);
        let err: ErrorObjectOwned = parse(&input, 1).unwrap_err().into();
        assert_eq!(err.code(), -38026);
    }

    #[test]
    fn call_limit_precedes_unsupported_keys() {
        let calls = vec!["{}"; MAX_SIMULATE_CALLS + 1].join(",");
        let input = format!(
            r#"{{"blockStateCalls":[{{"blockOverrides":{{"fooBar":"0x1"}},"calls":[{calls}]}}]}}"#
        );
        let err = parse(&input, 1).unwrap_err();
        assert!(
            matches!(&err, EthApiError::InvalidParams(msg)
                if msg == "too many calls in simulation payload"),
            "{err:?}"
        );
    }

    #[test]
    fn serializes_like_alloy_payload() {
        let payload = SimulatePayload::<TransactionRequest> {
            block_state_calls: vec![SimBlock::default()],
            trace_transfers: true,
            ..Default::default()
        };
        let original = serde_json::to_value(&payload).unwrap();
        let wrapped = serde_json::to_value(SimulateV1Request::from(payload)).unwrap();
        assert_eq!(wrapped, original);
    }

    #[test]
    fn forwards_raw_payload_without_changing_it() {
        let input = r#"{"blockStateCalls":[{"calls":[{}]}],"validation":true}"#;
        let wrapped = serde_json::from_str::<SimulateV1Request<TransactionRequest>>(input).unwrap();
        assert_eq!(serde_json::to_string(&wrapped).unwrap(), input);
    }
}
