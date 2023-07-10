//! bindings for state overrides in eth_call

use reth_primitives::{Address, Bytes, H256, U256, U64};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A set of account overrides
pub type StateOverride = HashMap<Address, AccountOverride>;

/// Custom account override used in call
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
#[allow(missing_docs)]
pub struct AccountOverride {
    /// Fake balance to set for the account before executing the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<U256>,
    /// Fake nonce to set for the account before executing the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<U64>,
    /// Fake EVM bytecode to inject into the account before executing the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Bytes>,
    /// Fake key-value mapping to override all slots in the account storage before executing the
    /// call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<HashMap<H256, H256>>,
    /// Fake key-value mapping to override individual slots in the account storage before executing
    /// the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_diff: Option<HashMap<H256, H256>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_override() {
        let s = r#"{
            "0x0000000000000000000000000000000000000124": {
                "code": "0x6080604052348015600e575f80fd5b50600436106026575f3560e01c80632096525514602a575b5f80fd5b60306044565b604051901515815260200160405180910390f35b5f604e600242605e565b5f0360595750600190565b505f90565b5f82607757634e487b7160e01b5f52601260045260245ffd5b50069056fea2646970667358221220287f77a4262e88659e3fb402138d2ee6a7ff9ba86bae487a95aa28156367d09c64736f6c63430008140033"
            }
        }"#;
        let state_override: StateOverride = serde_json::from_str(s).unwrap();
        let acc = state_override
            .get(&"0x0000000000000000000000000000000000000124".parse().unwrap())
            .unwrap();
        assert!(acc.code.is_some());
    }
}
