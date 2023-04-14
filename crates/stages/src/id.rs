use crate::stages::{BODIES, FINISH, HEADERS};
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
