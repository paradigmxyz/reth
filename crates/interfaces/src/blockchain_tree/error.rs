//! Error handling for the blockchain tree

use crate::{consensus::ConsensusError, Error};
use reth_primitives::{BlockHash, BlockNumber, SealedBlock, SealedBlockWithSenders};
use std::fmt::Formatter;

/// Error thrown when inserting a block failed because the block is considered invalid.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct InsertInvalidBlockError {
    inner: Box<InsertInvalidBlockData>,
}

// === impl InsertInvalidBlockError ===

impl InsertInvalidBlockError {
    /// Create a new InsertInvalidBlockError
    pub fn new(block: SealedBlock, kind: InsertInvalidBlockErrorKind) -> Self {
        Self { inner: InsertInvalidBlockData::boxed(block, kind) }
    }

    /// Create a new InsertInvalidBlockError from a consensus error
    pub fn consensus_error(error: ConsensusError, block: SealedBlock) -> Self {
        Self::new(block, InsertInvalidBlockErrorKind::Consensus(error))
    }

    /// Create a new InsertInvalidBlockError from a consensus error
    pub fn sender_recovery_error(block: SealedBlock) -> Self {
        Self::new(block, InsertInvalidBlockErrorKind::SenderRecovery)
    }

    /// Create a new InsertInvalidBlockError from an execution error
    pub fn execution_error(error: Error, block: SealedBlock) -> Self {
        Self::new(block, InsertInvalidBlockErrorKind::Execution(error))
    }

    /// Returns the error kind
    #[inline]
    pub fn kind(&self) -> &InsertInvalidBlockErrorKind {
        &self.inner.kind
    }

    /// Returns the invalid block.
    #[inline]
    pub fn block(&self) -> &SealedBlock {
        &self.inner.block
    }
}

#[derive(Debug)]
struct InsertInvalidBlockData {
    block: SealedBlock,
    kind: InsertInvalidBlockErrorKind,
}

impl std::fmt::Display for InsertInvalidBlockData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Failed to insert block {:?}: {}", self.block.hash, self.kind)
    }
}

impl std::error::Error for InsertInvalidBlockData {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.kind)
    }
}

impl InsertInvalidBlockData {
    fn new(block: SealedBlock, kind: InsertInvalidBlockErrorKind) -> Self {
        Self { block, kind }
    }

    fn boxed(block: SealedBlock, kind: InsertInvalidBlockErrorKind) -> Box<Self> {
        Box::new(Self::new(block, kind))
    }
}

/// All error variants possible when inserting a block
#[derive(Debug, thiserror::Error)]
pub enum InsertInvalidBlockErrorKind {
    /// Failed to recover senders for the block
    #[error("Failed to recover senders for block")]
    SenderRecovery,
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(ConsensusError),
    /// Block execution failed.
    #[error(transparent)]
    Execution(Error),

    #[error("Can't insert #{block_number} {block_hash} as last finalized block number is {last_finalized}")]
    PendingBlockIsFinalized {
        block_hash: BlockHash,
        block_number: BlockNumber,
        last_finalized: BlockNumber,
    },
    #[error("Database error")]
    DatabaseError,
}
