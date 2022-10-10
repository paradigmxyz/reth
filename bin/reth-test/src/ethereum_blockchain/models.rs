use reth_primitives::{Block as PrimitiveBlock, Bytes, Header as PrimitiveHeader, H256};
use serde;
use serde_json::{self, Error};
use std::collections::BTreeMap;

/// Blockchain test deserializer.
#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct Test(BTreeMap<String, BlockchainTestData>);

/// Ethereum blockchain test data
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockchainTestData {
    /// Genesis block header.
    #[serde(rename = "genesisBlockHeader")]
    pub genesis_block: Header,
    /// Genesis rlp.
    #[serde(rename = "genesisRLP")]
    pub genesis_rlp: Option<Bytes>,
    /// Blocks.
    pub blocks: Vec<Block>,
    /// Post state.
    pub post_state: Option<State>,
    /// Pre state.
    #[serde(rename = "pre")]
    pub pre_state: State,
    /// Hash of best block.
    #[serde(rename = "lastblockhash")]
    pub best_block: H256,
    /// Network.
    pub network: ForkSpec,
    #[serde(default)]
    #[serde(rename = "sealEngine")]
    /// Engine
    pub engine: SealEngine,
}

/// Ethereum blockchain test data Header.
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {}

/// Ethereum blockchain test data Block.
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {}

/// Ethereum blockchain test data State.
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {}


/// Ethereum blockchain test data State.
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSpec {}


/// Json Block test possible engine kind.
#[derive(Debug, PartialEq, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SealEngine {
    /// No consensus checks.
    #[default]
    NoProof,
}
