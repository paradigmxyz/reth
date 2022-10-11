use reth_db::{
    kv::{tables::SyncStage, tx::Tx, KVError},
    mdbx,
};
use reth_primitives::BlockNumber;
use std::fmt::Display;

/// The ID of a stage.
///
/// Each stage ID must be unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageId(pub &'static str);

impl Display for StageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl StageId {
    /// Get the last committed progress of this stage.
    pub fn get_progress<'db, K, E>(
        &self,
        tx: &Tx<'db, K, E>,
    ) -> Result<Option<BlockNumber>, KVError>
    where
        K: mdbx::TransactionKind,
        E: mdbx::EnvironmentKind,
    {
        tx.get::<SyncStage>(self.0.as_bytes().to_vec())
    }

    /// Save the progress of this stage.
    pub fn save_progress<'db, E>(
        &self,
        tx: &Tx<'db, mdbx::RW, E>,
        block: BlockNumber,
    ) -> Result<(), KVError>
    where
        E: mdbx::EnvironmentKind,
    {
        tx.put::<SyncStage>(self.0.as_bytes().to_vec(), block)
    }
}
