use crate::pipeline::PipelineEvent;
use reth_interfaces::{
    consensus, db::DatabaseError as DbError, executor, p2p::error::DownloadError,
    provider::ProviderError, RethError,
};
use reth_primitives::SealedHeader;
use thiserror::Error;
use tokio::sync::mpsc::error::SendError;

/// Represents the specific error type within a block error.
#[derive(Error, Debug)]
pub enum BlockErrorKind {
    /// The block encountered a validation error.
    #[error("validation error: {0}")]
    Validation(#[from] consensus::ConsensusError),
    /// The block encountered an execution error.
    #[error("execution error: {0}")]
    Execution(#[from] executor::BlockExecutionError),
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
        header_hash = header.hash,
        head_number = local_head.number,
        head_hash = local_head.hash,
    )]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: Box<SealedHeader>,
        /// The header we attempted to attach.
        header: Box<SealedHeader>,
        /// The error that occurred when attempting to attach the header.
        #[source]
        error: Box<consensus::ConsensusError>,
    },
    /// The headers stage is missing sync gap.
    #[error("missing sync gap")]
    MissingSyncGap,
    /// The stage encountered a database error.
    #[error("internal database error occurred: {0}")]
    Database(#[from] DbError),
    /// Invalid pruning configuration
    #[error(transparent)]
    PruningConfiguration(#[from] reth_primitives::PruneSegmentError),
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
    /// Internal error
    #[error(transparent)]
    Internal(#[from] RethError),
    /// The stage encountered a recoverable error.
    ///
    /// These types of errors are caught by the [Pipeline][crate::Pipeline] and trigger a restart
    /// of the stage.
    #[error(transparent)]
    Recoverable(Box<dyn std::error::Error + Send + Sync>),
    /// The stage encountered a fatal error.
    ///
    /// These types of errors stop the pipeline.
    #[error(transparent)]
    Fatal(Box<dyn std::error::Error + Send + Sync>),
}

impl StageError {
    /// If the error is fatal the pipeline will stop.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            StageError::Database(_) |
                StageError::Download(_) |
                StageError::DatabaseIntegrity(_) |
                StageError::StageCheckpoint(_) |
                StageError::MissingDownloadBuffer |
                StageError::MissingSyncGap |
                StageError::ChannelClosed |
                StageError::Fatal(_)
        )
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
    Database(#[from] DbError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The pipeline encountered an error while trying to send an event.
    #[error("pipeline encountered an error while trying to send an event")]
    Channel(#[from] SendError<PipelineEvent>),
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
