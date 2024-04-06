// TODO Move all types to alloy-rpc-types.

use std::collections::BTreeMap;

use alloy_primitives::{B256, TxHash, U256, U64};
use revm_primitives::SpecId;
use serde::{Deserialize, Deserializer, Serialize};

/// Wrapper type that ensures the type is named `params`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Params<T: Default> {
    #[serde(default)]
    pub params: T,
}

/// Represents the params to set forking which can take various forms
///  - untagged
///  - tagged `forking`
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forking {
    pub json_rpc_url: Option<String>,
    pub block_number: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub current_block_number: U64,
    pub current_block_timestamp: u64,
    pub current_block_hash: B256,
    pub hard_fork: SpecId,
    pub transaction_order: String,
    pub environment: NodeEnvironment,
    pub fork_config: NodeForkConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeEnvironment {
    pub base_fee: U256,
    pub chain_id: u64,
    pub gas_limit: U256,
    pub gas_price: U256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeForkConfig {
    pub fork_url: Option<String>,
    pub fork_block_number: Option<u64>,
    pub fork_retry_backoff: Option<u128>,
}


/// Anvil equivalent of `hardhat_metadata`.
/// Metadata about the current Anvil instance.
/// See <https://hardhat.org/hardhat-network/docs/reference#hardhat_metadata>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub client_version: String,
    pub chain_id: u64,
    pub instance_id: B256,
    pub latest_block_number: u64,
    pub latest_block_hash: B256,
    pub forked_network: Option<ForkedNetwork>,
    pub snapshots: BTreeMap<U256, (u64, B256)>,
}

/// Information about the forked network.
/// See <https://hardhat.org/hardhat-network/docs/reference#hardhat_metadata>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkedNetwork {
    pub chain_id: u64,
    pub fork_block_number: u64,
    pub fork_block_hash: TxHash,
}

/// Additional `evm_mine` options
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvmMineOptions {
    Options {
        #[serde(deserialize_with = "deserialize_stringified_u64_opt")]
        timestamp: Option<u64>,
        // If `blocks` is given, it will mine exactly blocks number of blocks, regardless of any
        // other blocks mined or reverted during it's operation
        blocks: Option<u64>,
    },
    /// The timestamp the block should be mined with
    #[serde(deserialize_with = "deserialize_stringified_u64_opt")]
    Timestamp(Option<u64>),
}

impl Default for EvmMineOptions {
    fn default() -> Self {
        EvmMineOptions::Options { timestamp: None, blocks: None }
    }
}

/// Supports parsing u64
///
/// See <https://github.com/gakonst/ethers-rs/issues/1507>
fn deserialize_stringified_u64_opt<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
{
    if let Some(num) = Option::<U256>::deserialize(deserializer)? {
        num.try_into().map(Some).map_err(serde::de::Error::custom)
    } else {
        Ok(None)
    }
}