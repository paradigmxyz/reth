//! Internal errors for the tree module.

use crate::tree::payload_processor::bal::BalExecutionError;
use alloy_consensus::BlockHeader;
use reth_consensus::ConsensusError;
pub use reth_engine_primitives::{
    InsertBlockErrorKind, InsertBlockFatalError, InsertBlockValidationError,
};
use reth_errors::ProviderError;
use reth_payload_primitives::NewPayloadError;
use reth_primitives_traits::{Block, BlockBody, SealedBlock};

/// This is an error that can come from advancing persistence.
#[derive(Debug, thiserror::Error)]
pub enum AdvancePersistenceError {
    /// The persistence channel was closed unexpectedly
    #[error("persistence channel closed")]
    ChannelClosed,
    /// A provider error
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

#[derive(thiserror::Error)]
#[error("Failed to insert block (hash={}, number={}, parent_hash={}): {}",
    .block.hash(),
    .block.number(),
    .block.parent_hash(),
    .kind)]
struct InsertBlockErrorData<B: Block> {
    block: SealedBlock<B>,
    #[source]
    kind: InsertBlockErrorKind,
}

impl<B: Block> std::fmt::Debug for InsertBlockErrorData<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertBlockError")
            .field("error", &self.kind)
            .field("hash", &self.block.hash())
            .field("number", &self.block.number())
            .field("parent_hash", &self.block.parent_hash())
            .field("num_txs", &self.block.body().transactions().len())
            .finish_non_exhaustive()
    }
}

impl<B: Block> InsertBlockErrorData<B> {
    const fn new(block: SealedBlock<B>, kind: InsertBlockErrorKind) -> Self {
        Self { block, kind }
    }

    fn boxed(block: SealedBlock<B>, kind: InsertBlockErrorKind) -> Box<Self> {
        Box::new(Self::new(block, kind))
    }
}

/// Error thrown when inserting a block failed because the block is considered invalid.
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct InsertBlockError<B: Block> {
    inner: Box<InsertBlockErrorData<B>>,
}

// === impl InsertBlockErrorTwo ===

impl<B: Block> InsertBlockError<B> {
    /// Create a new `InsertInvalidBlockErrorTwo`
    pub fn new(block: SealedBlock<B>, kind: InsertBlockErrorKind) -> Self {
        Self { inner: InsertBlockErrorData::boxed(block, kind) }
    }

    /// Create a new `InsertInvalidBlockError` from a consensus error
    pub fn consensus_error(error: ConsensusError, block: SealedBlock<B>) -> Self {
        Self::new(block, InsertBlockErrorKind::Consensus(error))
    }

    /// Consumes the error and returns the block that resulted in the error
    #[inline]
    pub fn into_block(self) -> SealedBlock<B> {
        self.inner.block
    }

    /// Returns the error kind
    #[inline]
    pub const fn kind(&self) -> &InsertBlockErrorKind {
        &self.inner.kind
    }

    /// Returns the block that resulted in the error
    #[inline]
    pub const fn block(&self) -> &SealedBlock<B> {
        &self.inner.block
    }

    /// Consumes the type and returns the block and error kind.
    #[inline]
    pub fn split(self) -> (SealedBlock<B>, InsertBlockErrorKind) {
        let inner = *self.inner;
        (inner.block, inner.kind)
    }
}

impl<B: Block> std::fmt::Debug for InsertBlockError<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.inner, f)
    }
}

impl From<BalExecutionError> for InsertBlockErrorKind {
    fn from(e: BalExecutionError) -> Self {
        match e {
            BalExecutionError::Consensus(inner) => Self::Consensus(inner),
            BalExecutionError::Execution(inner) => Self::Execution(inner),
            BalExecutionError::Provider(inner) => Self::Provider(inner),
            BalExecutionError::Other(inner) => Self::Other(inner),
        }
    }
}

/// Errors that may occur when inserting a payload.
#[derive(Debug, thiserror::Error)]
pub enum InsertPayloadError<B: Block> {
    /// Block validation error
    #[error(transparent)]
    Block(#[from] InsertBlockError<B>),
    /// Payload validation error
    #[error(transparent)]
    Payload(#[from] NewPayloadError),
}
