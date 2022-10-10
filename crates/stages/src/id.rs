use reth_db::mdbx;
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
        tx: &mdbx::Transaction<'db, K, E>,
    ) -> Result<Option<BlockNumber>, mdbx::Error>
    where
        K: mdbx::TransactionKind,
        E: mdbx::EnvironmentKind,
    {
        // TODO: Clean up when we get better database abstractions
        let bytes: Option<Vec<u8>> = tx.get(&tx.open_db(Some("SyncStage"))?, self.0.as_ref())?;

        Ok(bytes.map(|b| BlockNumber::from_be_bytes(b.try_into().expect("Database corrupt"))))
    }

    /// Save the progress of this stage.
    pub fn save_progress<'db, E>(
        &self,
        tx: &mdbx::Transaction<'db, mdbx::RW, E>,
        block: BlockNumber,
    ) -> Result<(), mdbx::Error>
    where
        E: mdbx::EnvironmentKind,
    {
        // TODO: Clean up when we get better database abstractions
        tx.put(
            &tx.open_db(Some("SyncStage"))?,
            self.0,
            block.to_be_bytes(),
            mdbx::WriteFlags::UPSERT,
        )
    }
}
