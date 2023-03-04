//! bindings for state overrides in eth_call

use reth_primitives::{Address, Bytes, H256, U256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A set of account overrides
pub type StateOverride = HashMap<Address, AccountOverride>;

/// Custom account override used in call
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
#[allow(missing_docs)]
pub struct AccountOverride {
    pub nonce: Option<u64>,
    pub code: Option<Bytes>,
    pub balance: Option<U256>,
    pub state: Option<HashMap<H256, H256>>,
    pub state_diff: Option<HashMap<H256, H256>>,
}
