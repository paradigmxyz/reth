use metrics::counter;
use reth_db::{
    tables::SyncStage,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
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
    pub fn get_progress<'db>(&self, tx: &impl DbTx<'db>) -> Result<Option<BlockNumber>, DbError> {
        tx.get::<SyncStage>(self.0.as_bytes().to_vec())
    }

    /// Save the progress of this stage.
    pub fn save_progress<'db>(
        &self,
        tx: &impl DbTxMut<'db>,
        block: BlockNumber,
    ) -> Result<(), DbError> {
        counter!("stage.progress", block, "stage" => self.0);
        tx.put::<SyncStage>(self.0.as_bytes().to_vec(), block)
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
}
