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
