use reth_primitives::{
    utils::serde_helpers::{deserialize_number, deserialize_stringified_u64},
    Address, Bytes, Header, H256, U256,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The genesis block specification.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genesis {
    /// The genesis header nonce.
    #[serde(deserialize_with = "deserialize_stringified_u64")]
    pub nonce: u64,
    /// The genesis header timestamp.
    #[serde(deserialize_with = "deserialize_stringified_u64")]
    pub timestamp: u64,
    /// The genesis header extra data.
    pub extra_data: Bytes,
    /// The genesis header gas limit.
    #[serde(deserialize_with = "deserialize_stringified_u64")]
    pub gas_limit: u64,
    /// The genesis header difficulty.
    #[serde(deserialize_with = "deserialize_number")]
    pub difficulty: U256,
    /// The genesis header mix hash.
    pub mix_hash: H256,
    /// The genesis header coinbase address.
    pub coinbase: Address,
    /// The genesis state root.
    pub state_root: H256,
    /// The initial state of accounts in the genesis block.
    pub alloc: HashMap<Address, GenesisAccount>,
}

impl From<Genesis> for Header {
    fn from(genesis: Genesis) -> Header {
        Header {
            gas_limit: genesis.gas_limit,
            difficulty: genesis.difficulty,
            nonce: genesis.nonce,
            extra_data: genesis.extra_data.0,
            state_root: genesis.state_root,
            timestamp: genesis.timestamp,
            mix_hash: genesis.mix_hash,
            beneficiary: genesis.coinbase,
            ..Default::default()
        }
    }
}

/// An account in the state of the genesis block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// The nonce of the account at genesis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<u64>,
    /// The balance of the account at genesis.
    pub balance: U256,
}
