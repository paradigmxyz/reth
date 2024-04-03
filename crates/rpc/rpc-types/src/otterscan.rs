#![allow(missing_docs)]

use crate::{
    serde_helpers::u64_hex, Block, BlockTransactions, Rich, Transaction, TransactionReceipt,
};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

/// Operation type enum for `InternalOperation` struct
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OperationType {
    /// Operation Transfer
    OpTransfer = 0,
    /// Operation Contract self destruct
    OpSelfDestruct = 1,
    /// Operation Create
    OpCreate = 2,
    /// Operation Create2
    OpCreate2 = 3,
}

/// Custom struct for otterscan `getInternalOperations` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InternalOperation {
    pub r#type: OperationType,
    pub from: Address,
    pub to: Address,
    pub value: U256,
}

/// Custom struct for otterscan `traceTransaction` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TraceEntry {
    pub r#type: String,
    pub depth: u32,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    pub input: Bytes,
}

/// Internal issuance struct for `BlockDetails` struct
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InternalIssuance {
    pub block_reward: U256,
    pub uncle_reward: U256,
    pub issuance: U256,
}

/// Custom `Block` struct that includes transaction count for Otterscan responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtsBlock {
    #[serde(flatten)]
    pub block: Block,
    pub transaction_count: usize,
}

/// Custom struct for otterscan `getBlockDetails` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDetails {
    pub block: OtsBlock,
    pub issuance: InternalIssuance,
    pub total_fees: U256,
}

/// Custom transaction receipt struct for otterscan `OtsBlockTransactions` struct
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtsTransactionReceipt {
    #[serde(flatten)]
    pub receipt: TransactionReceipt,
    #[serde(with = "u64_hex")]
    pub timestamp: u64,
}

/// Custom struct for otterscan `getBlockTransactions` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OtsBlockTransactions {
    pub fullblock: OtsBlock,
    pub receipts: Vec<OtsTransactionReceipt>,
}

/// Custom struct for otterscan `searchTransactionsAfter`and `searchTransactionsBefore` RPC
/// responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionsWithReceipts {
    pub txs: Vec<Transaction>,
    pub receipts: Vec<OtsTransactionReceipt>,
    pub first_page: bool,
    pub last_page: bool,
}

/// Custom struct for otterscan `getContractCreator` RPC responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractCreator {
    pub tx: Transaction,
    pub creator: Address,
}

impl From<Block> for OtsBlock {
    fn from(block: Block) -> Self {
        let transaction_count = match &block.transactions {
            BlockTransactions::Full(t) => t.len(),
            BlockTransactions::Hashes(t) => t.len(),
            BlockTransactions::Uncle => 0,
        };

        Self { block, transaction_count }
    }
}

impl From<Rich<Block>> for BlockDetails {
    fn from(rich_block: Rich<Block>) -> Self {
        Self {
            block: rich_block.inner.into(),
            issuance: Default::default(),
            total_fees: U256::default(),
        }
    }
}
