//! Validated RPC input for the options of `debug_traceCall`.

use crate::call_many::invalid_params;
use alloy_rpc_types_trace::geth::GethDebugTracingCallOptions;
use reth_rpc_eth_types::{block_overrides::deserialize_block_override_keys, EthApiError};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

/// The options of a `debug_traceCall` request, decoded only once their `blockOverrides` keys are
/// supported.
///
/// alloy's [`BlockOverrides`](alloy_rpc_types_eth::BlockOverrides) drops keys it does not know,
/// and RPC params are decoded before the handler runs. This keeps the raw input until
/// [`Self::into_inner`] has checked the keys with [`deserialize_block_override_keys`].
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum DebugTraceCallOptions {
    /// Raw RPC input, retained until the handler checks the block override keys.
    Raw(Box<RawValue>),
    /// Typed options supplied by an RPC client or internal caller.
    Parsed(Box<GethDebugTracingCallOptions>),
}

impl From<GethDebugTracingCallOptions> for DebugTraceCallOptions {
    fn from(opts: GethDebugTracingCallOptions) -> Self {
        Self::Parsed(Box::new(opts))
    }
}

impl<'de> Deserialize<'de> for DebugTraceCallOptions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Box::<RawValue>::deserialize(deserializer).map(Self::Raw)
    }
}

impl DebugTraceCallOptions {
    /// Decodes RPC input, then checks the keys of its `blockOverrides` object with
    /// [`deserialize_block_override_keys`].
    ///
    /// Returns [`EthApiError::InvalidParams`] if the input is malformed or has an unsupported
    /// block override key.
    pub fn into_inner(self) -> Result<GethDebugTracingCallOptions, EthApiError> {
        let raw = match self {
            Self::Raw(raw) => raw,
            Self::Parsed(opts) => return Ok(*opts),
        };
        // Decoded first, so input the typed decoder rejects keeps its error.
        let opts = serde_json::from_str(raw.get()).map_err(invalid_params)?;
        serde_json::from_str::<OptionsOverrideKeys>(raw.get()).map_err(invalid_params)?;
        Ok(opts)
    }
}

/// The `blockOverrides` keys of `debug_traceCall` options, checked while decoding. Other fields
/// are skipped.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(dead_code, reason = "fields are only decoded")]
struct OptionsOverrideKeys {
    #[serde(default, deserialize_with = "deserialize_block_override_keys")]
    block_overrides: (),
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee_types::error::ErrorObjectOwned;

    fn parse(input: &str) -> Result<GethDebugTracingCallOptions, EthApiError> {
        serde_json::from_str::<DebugTraceCallOptions>(input).unwrap().into_inner()
    }

    fn assert_unsupported_key(input: &str, key: &str) {
        let err = parse(input).unwrap_err();
        assert!(
            matches!(&err, EthApiError::InvalidParams(msg)
                if msg.starts_with(&format!("unsupported block override key: {key} at line 1 "))),
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
            (r#"{"time":"0x1","foo":"0x1"}"#, "foo"),
        ] {
            let input = format!(r#"{{"tracer":"callTracer","blockOverrides":{overrides}}}"#);
            assert_unsupported_key(&input, key);
        }
    }

    #[test]
    fn accepts_supported_block_override_keys() {
        let hash = format!("0x{}", "11".repeat(32));
        let address = format!("0x{}", "11".repeat(20));
        for overrides in [
            r#"{"number":"0x1","time":"0x2","baseFee":"0x3","random":"HASH","coinbase":"ADDR"}"#,
            r#"{"blockNumber":"0x1","timestamp":"0x2","feeRecipient":"ADDR","prevRandao":"HASH","baseFeePerGas":"0x3","parentBeaconBlockRoot":"HASH"}"#,
            r#"{"difficulty":"0x1","gasLimit":"0x2","blobBaseFee":"0x3","beaconRoot":"HASH","withdrawals":[],"blockHash":{"1":"HASH"}}"#,
            "{}",
            "null",
            r#"["0x1"]"#,
        ] {
            let overrides = overrides.replace("HASH", &hash).replace("ADDR", &address);
            let input = format!(r#"{{"blockOverrides":{overrides}}}"#);
            let expected = serde_json::from_str::<GethDebugTracingCallOptions>(&input).unwrap();
            assert_eq!(parse(&input).unwrap(), expected, "{input}");
        }
        assert!(parse(r#"{"blockOverrides":null}"#).unwrap().block_overrides.is_none());
    }

    #[test]
    fn decodes_like_alloy_options() {
        let address = format!("0x{}", "11".repeat(20));
        for input in [
            "{}".to_string(),
            r#"{"tracer":"callTracer","tracerConfig":{"onlyTopCall":true,"withLog":true},"timeout":"5s"}"#.to_string(),
            r#"{"disableStorage":true,"enableMemory":true,"enableReturnData":true}"#.to_string(),
            format!(
                r#"{{"tracer":"prestateTracer","tracerConfig":{{"diffMode":true}},"stateOverrides":{{"{address}":{{"balance":"0x1"}}}},"blockOverrides":{{"time":"0x2"}},"txIndex":"0x3"}}"#
            ),
            // Unknown keys outside `blockOverrides` are ignored, as alloy does.
            r#"{"fooBar":1,"BlockOverrides":{"fooBar":"0x1"}}"#.to_string(),
        ] {
            let expected = serde_json::from_str::<GethDebugTracingCallOptions>(&input).unwrap();
            assert_eq!(parse(&input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn rejects_malformed_input() {
        for input in [
            r#"{"tracer":1}"#,
            r#"{"txIndex":"zz"}"#,
            r#"{"blockOverrides":{"time":"zz"}}"#,
            r#"{"blockOverrides":"0x1"}"#,
            r#"[]"#,
            r#""callTracer""#,
        ] {
            let err = parse(input).unwrap_err();
            assert!(matches!(err, EthApiError::InvalidParams(_)), "{input}: {err:?}");
            let err: ErrorObjectOwned = err.into();
            assert_eq!(err.code(), -32602);
        }
    }

    #[test]
    fn serializes_like_alloy_options() {
        let input = r#"{"tracer":"callTracer","tracerConfig":{"onlyTopCall":true},"blockOverrides":{"time":"0x2"},"txIndex":"0x3"}"#;
        let opts = serde_json::from_str::<GethDebugTracingCallOptions>(input).unwrap();
        let original = serde_json::to_value(&opts).unwrap();
        let wrapped = serde_json::to_value(DebugTraceCallOptions::from(opts)).unwrap();
        assert_eq!(wrapped, original);
    }

    #[test]
    fn forwards_raw_options_without_changing_them() {
        let input = r#"{"tracer":"callTracer","blockOverrides":{"timestamp":"0x2"}}"#;
        let wrapped = serde_json::from_str::<DebugTraceCallOptions>(input).unwrap();
        assert_eq!(serde_json::to_string(&wrapped).unwrap(), input);
    }

    #[test]
    fn decodes_null_as_absent_options() {
        assert!(serde_json::from_str::<Option<DebugTraceCallOptions>>("null").unwrap().is_none());
    }
}
