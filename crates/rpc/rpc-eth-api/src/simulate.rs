//! Bounded RPC input for `eth_simulateV1`.

use alloy_rpc_types_eth::simulate::{SimBlock, SimulatePayload};
use serde::{
    de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{fmt, marker::PhantomData};

/// Maximum number of calls accepted in one `eth_simulateV1` request.
const MAX_SIMULATE_CALLS: usize = 4096;

/// Hard parsing ceiling; the configured execution limit is checked separately.
const MAX_SIMULATE_INPUT_BLOCKS: usize = 1024;

const TOO_MANY_CALLS: &str = "too many calls in eth_simulateV1 request";
const TOO_MANY_BLOCKS: &str = "too many blocks in eth_simulateV1 request";

/// An `eth_simulateV1` payload that bounds allocations while decoding RPC parameters.
///
/// Deserialization accepts at most 4096 calls across at most 1024 blocks. The configured
/// execution limit may be lower.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct SimulateV1Request<TxReq>(SimulatePayload<TxReq>);

impl<TxReq> SimulateV1Request<TxReq> {
    /// Returns the parsed simulation payload.
    pub fn into_inner(self) -> SimulatePayload<TxReq> {
        self.0
    }
}

impl<TxReq> From<SimulatePayload<TxReq>> for SimulateV1Request<TxReq> {
    fn from(payload: SimulatePayload<TxReq>) -> Self {
        Self(payload)
    }
}

impl<'de, TxReq: Deserialize<'de>> Deserialize<'de> for SimulateV1Request<TxReq> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", bound(deserialize = "TxReq: Deserialize<'de>"))]
        struct Input<TxReq> {
            #[serde(default)]
            block_state_calls: BoundedBlocks<TxReq>,
            #[serde(default)]
            trace_transfers: bool,
            #[serde(default)]
            validation: bool,
            #[serde(default)]
            return_full_transactions: bool,
        }

        let input = Input::deserialize(deserializer)?;
        Ok(Self(SimulatePayload {
            block_state_calls: input.block_state_calls.0,
            trace_transfers: input.trace_transfers,
            validation: input.validation,
            return_full_transactions: input.return_full_transactions,
        }))
    }
}

struct BoundedBlocks<TxReq>(Vec<SimBlock<TxReq>>);

impl<TxReq> Default for BoundedBlocks<TxReq> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, TxReq: Deserialize<'de>> Deserialize<'de> for BoundedBlocks<TxReq> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(BlocksVisitor(PhantomData))
    }
}

struct BlocksVisitor<TxReq>(PhantomData<TxReq>);

impl<'de, TxReq: Deserialize<'de>> Visitor<'de> for BlocksVisitor<TxReq> {
    type Value = BoundedBlocks<TxReq>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array of simulation blocks")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut blocks = Vec::new();
        let mut remaining_calls = MAX_SIMULATE_CALLS;
        while let Some(block) = seq.next_element_seed(BlockSeed {
            remaining_calls: &mut remaining_calls,
            allowed: blocks.len() < MAX_SIMULATE_INPUT_BLOCKS,
            marker: PhantomData,
        })? {
            blocks.push(block);
        }
        Ok(BoundedBlocks(blocks))
    }
}

struct BlockSeed<'a, TxReq> {
    remaining_calls: &'a mut usize,
    allowed: bool,
    marker: PhantomData<TxReq>,
}

impl<'de, TxReq: Deserialize<'de>> DeserializeSeed<'de> for BlockSeed<'_, TxReq> {
    type Value = SimBlock<TxReq>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        if !self.allowed {
            return Err(D::Error::custom(TOO_MANY_BLOCKS))
        }
        deserializer.deserialize_map(BlockVisitor {
            remaining_calls: self.remaining_calls,
            marker: PhantomData,
        })
    }
}

struct BlockVisitor<'a, TxReq> {
    remaining_calls: &'a mut usize,
    marker: PhantomData<TxReq>,
}

impl<'de, TxReq: Deserialize<'de>> Visitor<'de> for BlockVisitor<'_, TxReq> {
    type Value = SimBlock<TxReq>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a simulation block")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut block_overrides = None;
        let mut state_overrides = None;
        let mut calls = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "blockOverrides" => {
                    if block_overrides.is_some() {
                        return Err(A::Error::duplicate_field("blockOverrides"))
                    }
                    block_overrides = Some(map.next_value()?);
                }
                "stateOverrides" => {
                    if state_overrides.is_some() {
                        return Err(A::Error::duplicate_field("stateOverrides"))
                    }
                    state_overrides = Some(map.next_value()?);
                }
                "calls" => {
                    if calls.is_some() {
                        return Err(A::Error::duplicate_field("calls"))
                    }
                    calls = Some(map.next_value_seed(CallsSeed {
                        remaining_calls: &mut *self.remaining_calls,
                        marker: PhantomData,
                    })?);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }

        Ok(SimBlock {
            block_overrides: block_overrides.unwrap_or(None),
            state_overrides: state_overrides.unwrap_or(None),
            calls: calls.unwrap_or_default(),
        })
    }
}

struct CallsSeed<'a, TxReq> {
    remaining_calls: &'a mut usize,
    marker: PhantomData<TxReq>,
}

impl<'de, TxReq: Deserialize<'de>> DeserializeSeed<'de> for CallsSeed<'_, TxReq> {
    type Value = Vec<TxReq>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(CallsVisitor {
            remaining_calls: self.remaining_calls,
            marker: PhantomData,
        })
    }
}

struct CallsVisitor<'a, TxReq> {
    remaining_calls: &'a mut usize,
    marker: PhantomData<TxReq>,
}

impl<'de, TxReq: Deserialize<'de>> Visitor<'de> for CallsVisitor<'_, TxReq> {
    type Value = Vec<TxReq>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array of calls")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut calls = Vec::new();
        while let Some(call) = seq.next_element_seed(CallSeed {
            remaining_calls: &mut *self.remaining_calls,
            marker: PhantomData,
        })? {
            calls.push(call);
        }
        Ok(calls)
    }
}

struct CallSeed<'a, TxReq> {
    remaining_calls: &'a mut usize,
    marker: PhantomData<TxReq>,
}

impl<'de, TxReq: Deserialize<'de>> DeserializeSeed<'de> for CallSeed<'_, TxReq> {
    type Value = TxReq;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        if *self.remaining_calls == 0 {
            return Err(D::Error::custom(TOO_MANY_CALLS))
        }
        *self.remaining_calls -= 1;
        TxReq::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rpc_types_eth::TransactionRequest;

    fn payload_with_calls(counts: &[usize]) -> String {
        let blocks = counts
            .iter()
            .map(|&count| format!(r#"{{"calls":[{}]}}"#, vec!["{}"; count].join(",")))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"blockStateCalls":[{blocks}]}}"#)
    }

    #[test]
    fn accepts_call_limit() {
        let input = payload_with_calls(&[MAX_SIMULATE_CALLS]);
        let parsed = serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input)
            .unwrap()
            .into_inner();
        assert_eq!(parsed.block_state_calls[0].calls.len(), MAX_SIMULATE_CALLS);
    }

    #[test]
    fn rejects_calls_above_limit() {
        let input = payload_with_calls(&[MAX_SIMULATE_CALLS + 1]);
        let err =
            serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input).unwrap_err();
        assert_eq!(err.to_string().split(" at line").next(), Some(TOO_MANY_CALLS));
    }

    #[test]
    fn rejects_extra_call_before_deserializing_it() {
        let calls = vec!["{}"; MAX_SIMULATE_CALLS].join(",");
        let input = format!(r#"{{"blockStateCalls":[{{"calls":[{calls},123]}}]}}"#);
        let err =
            serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input).unwrap_err();
        assert_eq!(err.to_string().split(" at line").next(), Some(TOO_MANY_CALLS));
    }

    #[test]
    fn rejects_calls_split_across_blocks() {
        let input = payload_with_calls(&[MAX_SIMULATE_CALLS - 1, 2]);
        let err =
            serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input).unwrap_err();
        assert_eq!(err.to_string().split(" at line").next(), Some(TOO_MANY_CALLS));
    }

    #[test]
    fn rejects_blocks_above_limit() {
        let input = payload_with_calls(&vec![0; MAX_SIMULATE_INPUT_BLOCKS + 1]);
        let err =
            serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input).unwrap_err();
        assert_eq!(err.to_string().split(" at line").next(), Some(TOO_MANY_BLOCKS));
    }

    #[test]
    fn accepts_block_limit() {
        let input = payload_with_calls(&vec![0; MAX_SIMULATE_INPUT_BLOCKS]);
        let parsed = serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input)
            .unwrap()
            .into_inner();
        assert_eq!(parsed.block_state_calls.len(), MAX_SIMULATE_INPUT_BLOCKS);
    }

    #[test]
    fn rejects_extra_block_before_deserializing_it() {
        let blocks = vec!["{}"; MAX_SIMULATE_INPUT_BLOCKS].join(",");
        let input = format!(r#"{{"blockStateCalls":[{blocks},123]}}"#);
        let err =
            serde_json::from_str::<SimulateV1Request<TransactionRequest>>(&input).unwrap_err();
        assert_eq!(err.to_string().split(" at line").next(), Some(TOO_MANY_BLOCKS));
    }

    #[test]
    fn preserves_payload_fields() {
        let input = r#"{"blockStateCalls":[{"blockOverrides":null,"stateOverrides":null,"calls":[{}],"other":true},{}],"traceTransfers":true,"validation":true,"returnFullTransactions":true}"#;
        let expected = serde_json::from_str::<SimulatePayload<TransactionRequest>>(input).unwrap();
        let parsed = serde_json::from_str::<SimulateV1Request<TransactionRequest>>(input)
            .unwrap()
            .into_inner();
        assert_eq!(serde_json::to_value(parsed).unwrap(), serde_json::to_value(expected).unwrap());
    }

    #[test]
    fn rejects_duplicate_calls_field() {
        let input = r#"{"blockStateCalls":[{"calls":[],"calls":[]}]}"#;
        let err = serde_json::from_str::<SimulateV1Request<TransactionRequest>>(input).unwrap_err();
        assert_eq!(err.to_string().split(" at line").next(), Some("duplicate field `calls`"));
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
}
