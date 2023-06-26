use crate::PruneMode;
use serde::{Deserialize, Serialize};

/// Pruning configuration for the stages supporting it.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
pub struct StagePruneConfig {
    /// [SenderRecoveryStage](crate::stages::SenderRecoveryStage) pruning configuration.
    pub sender_recovery: Option<PruneMode>,
    /// [TransactionLookupStage](crate::stages::TransactionLookupStage) pruning configuration.
    pub transaction_lookup: Option<PruneMode>,
    /// [ExecutionStage](crate::stages::ExecutionStage) (only [Receipts](reth_db::tables::Receipts)
    /// table) pruning configuration.
    pub receipts: Option<PruneMode>,
    /// [IndexAccountHistoryStage](crate::stages::IndexAccountHistoryStage) and
    /// [ExecutionStage](crate::stages::ExecutionStage)
    /// (only [AccountChangeSet](reth_db::tables::AccountChangeSet) table) pruning configuration.
    pub account_history: Option<PruneMode>,
    /// [IndexStorageHistoryStage](crate::stages::IndexStorageHistoryStage) and
    /// [ExecutionStage](crate::stages::ExecutionStage)
    /// (only [StorageChangeSet](reth_db::tables::StorageChangeSet) table) pruning configuration.
    pub storage_history: Option<PruneMode>,
}
