use crate::stages::{
    ACCOUNT_HASHING, BODIES, EXECUTION, FINISH, HEADERS, INDEX_ACCOUNT_HISTORY,
    INDEX_STORAGE_HISTORY, MERKLE_EXECUTION, MERKLE_UNWIND, SENDER_RECOVERY, TOTAL_DIFFICULTY,
    TRANSACTION_LOOKUP,
};
use reth_db::{
    tables::SyncStage,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::BlockNumber;
use std::fmt::Display;

/// All known stages
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum StageKind {
    Headers,
    Bodies,
    SenderRecovery,
    TotalDifficulty,
    AccountHashing,
    StorageHashing,
    IndexAccountHistory,
    IndexStorageHistory,
    MerkleExecution,
    MerkleUnwind,
    Execution,
    TransactionLookup,
    Finish,
}

impl StageKind {
    /// All supported Stages
    pub const ALL: [StageKind; 13] = [
        StageKind::Headers,
        StageKind::Bodies,
        StageKind::SenderRecovery,
        StageKind::TotalDifficulty,
        StageKind::AccountHashing,
        StageKind::StorageHashing,
        StageKind::IndexAccountHistory,
        StageKind::IndexStorageHistory,
        StageKind::MerkleExecution,
        StageKind::MerkleUnwind,
        StageKind::Execution,
        StageKind::TransactionLookup,
        StageKind::Finish,
    ];

    /// Returns the ID of this stage.
    pub fn id(&self) -> StageId {
        match self {
            StageKind::Headers => HEADERS,
            StageKind::Bodies => BODIES,
            StageKind::SenderRecovery => SENDER_RECOVERY,
            StageKind::TotalDifficulty => TOTAL_DIFFICULTY,
            StageKind::AccountHashing => ACCOUNT_HASHING,
            StageKind::StorageHashing => ACCOUNT_HASHING,
            StageKind::IndexAccountHistory => INDEX_ACCOUNT_HISTORY,
            StageKind::IndexStorageHistory => INDEX_STORAGE_HISTORY,
            StageKind::MerkleExecution => MERKLE_EXECUTION,
            StageKind::MerkleUnwind => MERKLE_UNWIND,
            StageKind::Execution => EXECUTION,
            StageKind::TransactionLookup => TRANSACTION_LOOKUP,
            StageKind::Finish => FINISH,
        }
    }
}

impl Display for StageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id())
    }
}

/// The ID of a stage.
///
/// Each stage ID must be unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StageId(pub &'static str);

impl Display for StageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl StageId {
    /// Returns true if it's a downloading stage [HEADERS] or [BODIES
    pub fn is_downloading_stage(&self) -> bool {
        *self == HEADERS || *self == BODIES
    }

    /// Returns true indicating if it's the finish stage [FINISH]
    pub fn is_finish(&self) -> bool {
        *self == FINISH
    }

    /// Get the last committed progress of this stage.
    pub fn get_progress<'db>(&self, tx: &impl DbTx<'db>) -> Result<Option<BlockNumber>, DbError> {
        tx.get::<SyncStage>(self.0.to_string())
    }

    /// Save the progress of this stage.
    pub fn save_progress<'db>(
        &self,
        tx: &impl DbTxMut<'db>,
        block: BlockNumber,
    ) -> Result<(), DbError> {
        tx.put::<SyncStage>(self.0.to_string(), block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_id_display() {
        assert_eq!(StageId("foo").to_string(), "foo");
        assert_eq!(StageId("bar").to_string(), "bar");
    }

    #[test]
    fn is_downloading_stage() {
        assert!(HEADERS.is_downloading_stage());
        assert!(BODIES.is_downloading_stage());
    }
}
