use crate::PipelineEvent;
use alloy_primitives::TxNumber;
use reth_consensus::ConsensusError;
use reth_errors::{BlockExecutionError, DatabaseError, RethError};
use reth_network_p2p::error::DownloadError;
use reth_primitives_traits::SealedHeader;
use reth_provider::ProviderError;
use reth_prune::{PruneSegment, PruneSegmentError, PrunerError};
use reth_static_file_types::StaticFileSegment;
use thiserror::Error;
use tokio::sync::broadcast::error::SendError;

/// Represents the specific error type within a block error.
#[derive(Error, Debug)]
pub enum BlockErrorKind {
    /// The block encountered a validation error.
    #[error("validation error: {0}")]
    Validation(#[from] ConsensusError),
    /// The block encountered an execution error.
    #[error("execution error: {0}")]
    Execution(#[from] BlockExecutionError),
}

impl BlockErrorKind {
    /// Returns `true` if the error is a state root error.
    pub const fn is_state_root_error(&self) -> bool {
        match self {
            Self::Validation(err) => err.is_state_root_error(),
            Self::Execution(err) => err.is_state_root_error(),
        }
    }
}

/// A stage execution error.
#[derive(Error, Debug)]
pub enum StageError {
    /// The stage encountered an error related to a block.
    #[error("stage encountered an error in block #{number}: {error}", number = block.number)]
    Block {
        /// The block that caused the error.
        block: Box<SealedHeader>,
        /// The specific error type, either consensus or execution error.
        #[source]
        error: BlockErrorKind,
    },
    /// The stage encountered a downloader error where the responses cannot be attached to the
    /// current head.
    #[error(
        "stage encountered inconsistent chain: \
         downloaded header #{header_number} ({header_hash}) is detached from \
         local head #{head_number} ({head_hash}): {error}",
        header_number = header.number,
        header_hash = header.hash(),
        head_number = local_head.number,
        head_hash = local_head.hash(),
    )]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: Box<SealedHeader>,
        /// The header we attempted to attach.
        header: Box<SealedHeader>,
        /// The error that occurred when attempting to attach the header.
        #[source]
        error: Box<ConsensusError>,
    },
    /// The headers stage is missing sync gap.
    #[error("missing sync gap")]
    MissingSyncGap,
    /// The stage encountered a database error.
    #[error("internal database error occurred: {0}")]
    Database(#[from] DatabaseError),
    /// Invalid pruning configuration
    #[error(transparent)]
    PruningConfiguration(#[from] PruneSegmentError),
    /// Pruner error
    #[error(transparent)]
    Pruner(#[from] PrunerError),
    /// Invalid checkpoint passed to the stage
    #[error("invalid stage checkpoint: {0}")]
    StageCheckpoint(u64),
    /// Missing download buffer on stage execution.
    /// Returned if stage execution was called without polling for readiness.
    #[error("missing download buffer")]
    MissingDownloadBuffer,
    /// Download channel closed
    #[error("download channel closed")]
    ChannelClosed,
    /// The stage encountered a database integrity error.
    #[error("database integrity error occurred: {0}")]
    DatabaseIntegrity(#[from] ProviderError),
    /// Invalid download response. Applicable for stages which
    /// rely on external downloaders
    #[error("invalid download response: {0}")]
    Download(#[from] DownloadError),
    /// Database is ahead of static file data.
    #[error("missing static file data for block number: {number}", number = block.number)]
    MissingStaticFileData {
        /// Starting block with  missing data.
        block: Box<SealedHeader>,
        /// Static File segment
        segment: StaticFileSegment,
    },
    /// Unrecoverable inconsistency error related to a transaction number in a static file segment.
    #[error(
        "inconsistent transaction number for {segment}. db: {database}, static_file: {static_file}"
    )]
    InconsistentTxNumber {
        /// Static File segment where this error was encountered.
        segment: StaticFileSegment,
        /// Expected database transaction number.
        database: TxNumber,
        /// Expected static file transaction number.
        static_file: TxNumber,
    },
    /// The prune checkpoint for the given segment is missing.
    #[error("missing prune checkpoint for {0}")]
    MissingPruneCheckpoint(PruneSegment),
    /// Post Execute Commit error
    #[error("post execute commit error occurred: {_0}")]
    PostExecuteCommit(&'static str),
    /// Internal error
    #[error(transparent)]
    Internal(#[from] RethError),
    /// The stage encountered a recoverable error.
    ///
    /// These types of errors are caught by the [Pipeline][crate::Pipeline] and trigger a restart
    /// of the stage.
    #[error(transparent)]
    Recoverable(Box<dyn core::error::Error + Send + Sync>),
    /// The stage encountered a fatal error.
    ///
    /// These types of errors stop the pipeline.
    #[error(transparent)]
    Fatal(Box<dyn core::error::Error + Send + Sync>),
}

impl StageError {
    /// If the error is fatal the pipeline will stop.
    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Database(_) |
                Self::Download(_) |
                Self::DatabaseIntegrity(_) |
                Self::StageCheckpoint(_) |
                Self::MissingDownloadBuffer |
                Self::MissingSyncGap |
                Self::ChannelClosed |
                Self::InconsistentTxNumber { .. } |
                Self::Internal(_) |
                Self::Fatal(_)
        )
    }
}

impl From<std::io::Error> for StageError {
    fn from(source: std::io::Error) -> Self {
        Self::Fatal(Box::new(source))
    }
}

/// A pipeline execution error.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// The pipeline encountered an irrecoverable error in one of the stages.
    #[error(transparent)]
    Stage(#[from] StageError),
    /// The pipeline encountered a database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The pipeline encountered an error while trying to send an event.
    #[error("pipeline encountered an error while trying to send an event")]
    Channel(#[from] Box<SendError<PipelineEvent>>),
    /// Internal error
    #[error(transparent)]
    Internal(#[from] RethError),
    /// The pipeline encountered an unwind when `fail_on_unwind` was set to `true`.
    #[error("unexpected unwind")]
    UnexpectedUnwind,
}
