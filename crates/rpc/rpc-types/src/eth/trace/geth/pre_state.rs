use reth_primitives::{serde_helper::num::from_int_or_hex_opt, Address, H256, U256};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// <https://github.com/ethereum/go-ethereum/blob/91cb6f863a965481e51d5d9c0e5ccd54796fd967/eth/tracers/native/prestate.go#L38>
#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PreStateFrame {
    Default(PreStateMode),
    Diff(DiffMode),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreStateMode(pub BTreeMap<Address, AccountState>);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffMode {
    pub pre: BTreeMap<Address, AccountState>,
    pub post: BTreeMap<Address, AccountState>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountState {
    #[serde(
        default,
        deserialize_with = "from_int_or_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub balance: Option<U256>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(
        default,
        deserialize_with = "from_int_or_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub nonce: Option<U256>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<BTreeMap<H256, H256>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreStateConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_mode: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::geth::*;

    // See <https://github.com/ethereum/go-ethereum/tree/master/eth/tracers/internal/tracetest/testdata>
    const DEFAULT: &str = include_str!("../../../../test_data/pre_state_tracer/default.json");
    const LEGACY: &str = include_str!("../../../../test_data/pre_state_tracer/legacy.json");
    const DIFF_MODE: &str = include_str!("../../../../test_data/pre_state_tracer/diff_mode.json");

    #[test]
    fn test_serialize_pre_state_trace() {
        let mut opts = GethDebugTracingCallOptions::default();
        opts.tracing_options.config.disable_storage = Some(false);
        opts.tracing_options.tracer =
            Some(GethDebugTracerType::BuiltInTracer(GethDebugBuiltInTracerType::PreStateTracer));
        opts.tracing_options.tracer_config =
            serde_json::to_value(PreStateConfig { diff_mode: Some(true) }).unwrap().into();

        assert_eq!(
            serde_json::to_string(&opts).unwrap(),
            r#"{"disableStorage":false,"tracer":"prestateTracer","tracerConfig":{"diffMode":true}}"#
        );
    }

    #[test]
    fn test_deserialize_pre_state_trace() {
        let trace: PreStateFrame = serde_json::from_str(DEFAULT).unwrap();
        match trace {
            PreStateFrame::Default(PreStateMode(_)) => {}
            _ => unreachable!(),
        }
        let _trace: PreStateFrame = serde_json::from_str(LEGACY).unwrap();
        let trace: PreStateFrame = serde_json::from_str(DIFF_MODE).unwrap();
        match trace {
            PreStateFrame::Diff(DiffMode { pre: _pre, post: _post }) => {}
            _ => unreachable!(),
        }
    }
}
