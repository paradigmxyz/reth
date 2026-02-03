//! Consistency validation for storage providers.

use alloy_primitives::BlockNumber;
use reth_static_file_types::StaticFileSegment;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsistencyError {
    #[error(
        "static file {segment:?} has gap: database starts at {db_first}, \
         static file ends at {static_file_highest}"
    )]
    StaticFileGap {
        segment: StaticFileSegment,
        db_first: u64,
        static_file_highest: u64,
        unwind_target: BlockNumber,
    },

    #[error(
        "static file {segment:?} behind checkpoint: checkpoint={checkpoint}, \
         static_file={static_file_block}"
    )]
    CheckpointAheadOfStaticFile {
        segment: StaticFileSegment,
        checkpoint: BlockNumber,
        static_file_block: BlockNumber,
    },

    #[error(
        "static file {segment:?} ahead of checkpoint: static_file={static_file_block}, \
         checkpoint={checkpoint}"
    )]
    StaticFileAheadOfCheckpoint {
        segment: StaticFileSegment,
        static_file_block: BlockNumber,
        checkpoint: BlockNumber,
    },

    #[error("rocksdb {table} inconsistent: tip={tip}, checkpoint={checkpoint}")]
    RocksDBCheckpointMismatch {
        table: &'static str,
        tip: BlockNumber,
        checkpoint: BlockNumber,
    },

    #[error("rocksdb {table} has data but checkpoint is 0")]
    RocksDBUnexpectedData { table: &'static str },

    #[error("static file tip ({static_file_tip}) behind rocksdb checkpoint ({checkpoint}) for {table}")]
    StaticFileBehindRocksDB {
        table: &'static str,
        static_file_tip: BlockNumber,
        checkpoint: BlockNumber,
    },

    #[error("static file {segment:?} corrupted: {details}")]
    FileCorruption { segment: StaticFileSegment, details: String },
}

impl ConsistencyError {
    pub const fn unwind_target(&self) -> Option<BlockNumber> {
        match self {
            Self::StaticFileGap { unwind_target, .. } => Some(*unwind_target),
            Self::CheckpointAheadOfStaticFile { static_file_block, .. } => Some(*static_file_block),
            Self::StaticFileBehindRocksDB { static_file_tip, .. } => Some(*static_file_tip),
            Self::StaticFileAheadOfCheckpoint { .. } |
            Self::RocksDBCheckpointMismatch { .. } |
            Self::RocksDBUnexpectedData { .. } |
            Self::FileCorruption { .. } => None,
        }
    }

    pub const fn is_healable_in_place(&self) -> bool {
        matches!(
            self,
            Self::StaticFileAheadOfCheckpoint { .. } |
                Self::RocksDBCheckpointMismatch { .. } |
                Self::RocksDBUnexpectedData { .. }
        )
    }
}

#[derive(Debug, Default)]
pub struct ConsistencyReport {
    pub errors: Vec<ConsistencyError>,
}

impl ConsistencyReport {
    pub fn is_consistent(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn suggested_unwind_target(&self) -> Option<BlockNumber> {
        self.errors.iter().filter_map(|e| e.unwind_target()).min()
    }

    pub fn unwind_required_errors(&self) -> impl Iterator<Item = &ConsistencyError> {
        self.errors.iter().filter(|e| e.unwind_target().is_some())
    }

    pub fn healable_errors(&self) -> impl Iterator<Item = &ConsistencyError> {
        self.errors.iter().filter(|e| e.is_healable_in_place())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_report_is_consistent() {
        let report = ConsistencyReport::default();
        assert!(report.is_consistent());
        assert!(report.suggested_unwind_target().is_none());
    }

    #[test]
    fn test_unwind_targets() {
        let gap = ConsistencyError::StaticFileGap {
            segment: StaticFileSegment::Headers,
            db_first: 100,
            static_file_highest: 50,
            unwind_target: 50,
        };
        assert_eq!(gap.unwind_target(), Some(50));
        assert!(!gap.is_healable_in_place());

        let ahead = ConsistencyError::StaticFileAheadOfCheckpoint {
            segment: StaticFileSegment::Headers,
            static_file_block: 100,
            checkpoint: 90,
        };
        assert!(ahead.unwind_target().is_none());
        assert!(ahead.is_healable_in_place());
    }

    #[test]
    fn test_suggested_unwind_takes_minimum() {
        let mut report = ConsistencyReport::default();
        report.errors.push(ConsistencyError::StaticFileGap {
            segment: StaticFileSegment::Headers,
            db_first: 100,
            static_file_highest: 50,
            unwind_target: 50,
        });
        report.errors.push(ConsistencyError::CheckpointAheadOfStaticFile {
            segment: StaticFileSegment::Transactions,
            checkpoint: 80,
            static_file_block: 30,
        });

        assert_eq!(report.suggested_unwind_target(), Some(30));
    }
}
