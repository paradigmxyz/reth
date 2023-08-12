use crate::{Block, BlockTransactions, Rich, Transaction, TransactionReceipt};
use reth_primitives::{Address, Bytes, U256};
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
    r#type: OperationType,
    from: Address,
    to: Address,
    value: u128,
}

/// Custom struct for otterscan `traceTransaction` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TraceEntry {
    r#type: String,
    depth: u32,
    from: Address,
    to: Address,
    value: u128,
    input: Bytes,
}

/// Internal issuance struct for `BlockDetails` struct
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InternalIssuance {
    block_reward: U256,
    uncle_reward: U256,
    issuance: U256,
}

/// Custom `Block` struct that includes transaction count for Otterscan responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtsBlock {
    #[serde(flatten)]
    block: Block,
    transaction_count: usize,
}

/// Custom struct for otterscan `getBlockDetails` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDetails {
    block: OtsBlock,
    issuance: InternalIssuance,
    total_fees: U256,
}

/// Custom transaction receipt struct for otterscan `OtsBlockTransactions` struct
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtsTransactionReceipt {
    #[serde(flatten)]
    receipt: TransactionReceipt,
    timestamp: u64,
}

/// Custom struct for otterscan `getBlockTransactions` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OtsBlockTransactions {
    fullblock: OtsBlock,
    receipts: Vec<OtsTransactionReceipt>,
}

/// Custom struct for otterscan `searchTransactionsAfter`and `searchTransactionsBefore` RPC
/// responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionsWithReceipts {
    txs: Vec<Transaction>,
    receipts: Vec<OtsTransactionReceipt>,
    first_page: bool,
    last_page: bool,
}

/// Custom struct for otterscan `getContractCreator` RPC responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractCreator {
    tx: Transaction,
    creator: Address,
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
