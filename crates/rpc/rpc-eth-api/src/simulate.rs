//! Bounded RPC input for `eth_simulateV1`.

use alloy_rpc_types_eth::simulate::{
    SimulatePayload, SimulatePayloadLimitExceeded, SimulatePayloadSeed,
};
use reth_rpc_eth_types::{simulate::EthSimulateError, EthApiError};
use serde::{
    de::{DeserializeOwned, DeserializeSeed},
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
        match payload {
            Ok(payload) => Ok(payload),
            Err(SimulatePayloadLimitExceeded::Blocks) => {
                Err(EthApiError::other(EthSimulateError::TooManyBlocks))
            }
            Err(err) => Err(EthApiError::InvalidParams(err.to_string())),
        }
    }
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
