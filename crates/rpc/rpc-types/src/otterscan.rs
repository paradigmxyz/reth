use crate::{Block, Transaction, TransactionReceipt};
use reth_primitives::{Address, Bytes};
use serde::{Deserialize, Serialize};

/// Operation type enum for `InternalOperation` struct
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OperationType {
    /// Operation Transfer
    OpTransfer,
    /// Operation Contract self destruct
    OpSelfDestruct,
    /// Operation Create
    OpCreate,
    /// Operation Create2
    OpCreate2,
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalIssuance {
    block_reward: String,
    uncle_reward: String,
    issuance: String,
}

/// Custom struct for otterscan `getBlockDetails` RPC response
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDetails {
    block: Block,
    issuance: InternalIssuance,
    total_fees: u64,
}

/// Custom struct for otterscan `searchTransactionsAfter`and `searchTransactionsBefore` RPC
/// responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionsWithReceipts {
    txs: Vec<Transaction>,
    receipts: TransactionReceipt,
    first_page: bool,
    last_page: bool,
}

/// Custom struct for otterscan `getContractCreator` RPC responses
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractCreator {
    tx: Transaction,
    creator: Address,
}
