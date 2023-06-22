use crate::PruneMode;
use serde::{Deserialize, Serialize};

/// Pruning configuration for each stage supporting it.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
pub struct PruneConfig {
    /// Sender Recovery stage pruning configuration.
    pub sender_recovery: Option<PruneMode>,
    /// Execution stage pruning configuration.
    pub execution: Option<PruneMode>,
    /// Transaction Lookup stage pruning configuration.
    pub transaction_lookup: Option<PruneMode>,
    /// Index Account History stage pruning configuration.
    pub index_account_history: Option<PruneMode>,
    /// Index Storage History stage pruning configuration.
    pub index_storage_history: Option<PruneMode>,
}
