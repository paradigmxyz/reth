use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use reth_interfaces::Result;
use reth_primitives::{Account, Address, Bytes, Transaction, H256, U256, U64};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrestateAccount {
    pub balance: U256,
    pub nonce: U64,
    pub storage: HashMap<H256, U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Bytes>,
}

// todo: support rest of params
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrestateEnv {
    pub current_coinbase: Address,
    pub current_difficulty: U256,
    pub current_number: U64,
    pub current_timestamp: U256,
    pub current_gas_limit: U256,
}

use serde::Serializer;

fn geth_alloc_compat<S>(
    value: &std::collections::BTreeMap<U256, U256>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_map(
        value.iter().map(|(k, v)| (format!("0x{:0>64x}", k), format!("0x{:0>64x}", v))),
    )
}
