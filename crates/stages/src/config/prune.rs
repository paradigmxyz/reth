use crate::{
    stages::{
        ExecutionStage, IndexAccountHistoryStage, IndexStorageHistoryStage, SenderRecoveryStage,
        TransactionLookupStage,
    },
    PruneMode,
};
use reth_db::tables::{AccountChangeSet, Receipts, StorageChangeSet};
use serde::{Deserialize, Serialize};

/// Pruning configuration for the stages supporting it.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
pub struct StagePruneConfig {
    /// [SenderRecoveryStage] pruning configuration.
    pub sender_recovery: Option<PruneMode>,
    /// [TransactionLookupStage] pruning configuration.
    pub transaction_lookup: Option<PruneMode>,
    /// [ExecutionStage] (only [Receipts] table) pruning configuration.
    pub receipts: Option<PruneMode>,
    /// [IndexAccountHistoryStage] and [ExecutionStage] (only [AccountChangeSet] table)
    /// pruning configuration.
    pub account_history: Option<PruneMode>,
    /// [IndexStorageHistoryStage] and [ExecutionStage] (only [StorageChangeSet] table)
    /// pruning configuration.
    pub storage_history: Option<PruneMode>,
}
