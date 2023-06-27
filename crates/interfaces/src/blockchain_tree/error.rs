//! Error handling for the blockchain tree

use crate::{
    consensus::ConsensusError,
    executor::{BlockExecutionError, BlockValidationError},
};
use reth_primitives::{BlockHash, BlockNumber, SealedBlock};

/// Various error cases that can occur when a block violates tree assumptions.
#[derive(Debug, Clone, Copy, thiserror::Error, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum BlockchainTreeError {
    /// Thrown if the block number is lower than the last finalized block number.
    #[error("Block number is lower than the last finalized block number #{last_finalized}")]
    PendingBlockIsFinalized {
        /// The block number of the last finalized block.
        last_finalized: BlockNumber,
    },
    /// Thrown if no side chain could be found for the block.
    #[error("BlockChainId can't be found in BlockchainTree with internal index {chain_id}")]
    BlockSideChainIdConsistency {
        /// The internal identifier for the side chain.
        chain_id: u64,
    },
    #[error("Canonical chain header #{block_hash} can't be found ")]
    CanonicalChain { block_hash: BlockHash },
    #[error("Block number #{block_number} not found in blockchain tree chain")]
    BlockNumberNotFoundInChain { block_number: BlockNumber },
    #[error("Block hash {block_hash} not found in blockchain tree chain")]
    BlockHashNotFoundInChain { block_hash: BlockHash },
    // Thrown if the block failed to buffer
    #[error("Block with hash {block_hash:?} failed to buffer")]
    BlockBufferingFailed { block_hash: BlockHash },
}

/// Error thrown when inserting a block failed because the block is considered invalid.
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct InsertBlockError {
    inner: Box<InsertBlockErrorData>,
}

// === impl InsertBlockError ===

impl InsertBlockError {
    /// Create a new InsertInvalidBlockError
    pub fn new(block: SealedBlock, kind: InsertBlockErrorKind) -> Self {
        Self { inner: InsertBlockErrorData::boxed(block, kind) }
    }

    /// Create a new InsertInvalidBlockError from a tree error
    pub fn tree_error(error: BlockchainTreeError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::Tree(error))
    }

    /// Create a new InsertInvalidBlockError from a consensus error
    pub fn consensus_error(error: ConsensusError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::Consensus(error))
    }

    /// Create a new InsertInvalidBlockError from a consensus error
    pub fn sender_recovery_error(block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::SenderRecovery)
    }

    /// Create a new InsertInvalidBlockError from an execution error
    pub fn execution_error(error: BlockExecutionError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::Execution(error))
    }

    /// Consumes the error and returns the block that resulted in the error
    #[inline]
    pub fn into_block(self) -> SealedBlock {
        self.inner.block
    }

    /// Returns the error kind
    #[inline]
    pub fn kind(&self) -> &InsertBlockErrorKind {
        &self.inner.kind
    }

    /// Returns the block that resulted in the error
    #[inline]
    pub fn block(&self) -> &SealedBlock {
        &self.inner.block
    }

    /// Consumes the type and returns the block and error kind.
    #[inline]
    pub fn split(self) -> (SealedBlock, InsertBlockErrorKind) {
        let inner = *self.inner;
        (inner.block, inner.kind)
    }
}

impl std::fmt::Debug for InsertBlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.inner, f)
    }
}

struct InsertBlockErrorData {
    block: SealedBlock,
    kind: InsertBlockErrorKind,
}

impl std::fmt::Display for InsertBlockErrorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Failed to insert block (hash={:?}, number={}, parent_hash={:?}): {}",
            self.block.hash, self.block.number, self.block.parent_hash, self.kind
        )
    }
}

impl std::fmt::Debug for InsertBlockErrorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertBlockError")
            .field("error", &self.kind)
            .field("hash", &self.block.hash)
            .field("number", &self.block.number)
            .field("parent_hash", &self.block.parent_hash)
            .field("num_txs", &self.block.body.len())
            .finish_non_exhaustive()
    }
}

impl std::error::Error for InsertBlockErrorData {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.kind)
    }
}

impl InsertBlockErrorData {
    fn new(block: SealedBlock, kind: InsertBlockErrorKind) -> Self {
        Self { block, kind }
    }

    fn boxed(block: SealedBlock, kind: InsertBlockErrorKind) -> Box<Self> {
        Box::new(Self::new(block, kind))
    }
}

/// All error variants possible when inserting a block
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockErrorKind {
    /// Failed to recover senders for the block
    #[error("Failed to recover senders for block")]
    SenderRecovery,
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(ConsensusError),
    /// Block execution failed.
    #[error(transparent)]
    Execution(BlockExecutionError),
    /// Block violated tree invariants.
    #[error(transparent)]
    Tree(#[from] BlockchainTreeError),
    /// An internal error occurred, like interacting with the database.
    #[error("Internal error")]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl InsertBlockErrorKind {
    /// Returns true if the error is a tree error
    pub fn is_tree_error(&self) -> bool {
        matches!(self, InsertBlockErrorKind::Tree(_))
    }

    /// Returns true if the error is a consensus error
    pub fn is_consensus_error(&self) -> bool {
        matches!(self, InsertBlockErrorKind::Consensus(_))
    }

    /// Returns true if the error is caused by an invalid block
    ///
    /// This is intended to be used to determine if the block should be marked as invalid.
    pub fn is_invalid_block(&self) -> bool {
        match self {
            InsertBlockErrorKind::SenderRecovery | InsertBlockErrorKind::Consensus(_) => true,
            // other execution errors that are considered internal errors
            InsertBlockErrorKind::Execution(err) => {
                match err {
                    BlockExecutionError::Validation(_) => {
                        // this is caused by an invalid block
                        true
                    }
                    // these are internal errors, not caused by an invalid block
                    BlockExecutionError::ProviderError |
                    BlockExecutionError::CanonicalRevert { .. } |
                    BlockExecutionError::CanonicalCommit { .. } |
                    BlockExecutionError::BlockHashNotFoundInChain { .. } |
                    BlockExecutionError::AppendChainDoesntConnect { .. } |
                    BlockExecutionError::UnavailableForTest => false,
                }
            }
            InsertBlockErrorKind::Tree(err) => {
                match err {
                    BlockchainTreeError::PendingBlockIsFinalized { .. } => {
                        // the block's number is lower than the finalized block's number
                        true
                    }
                    BlockchainTreeError::BlockSideChainIdConsistency { .. } |
                    BlockchainTreeError::CanonicalChain { .. } |
                    BlockchainTreeError::BlockNumberNotFoundInChain { .. } |
                    BlockchainTreeError::BlockHashNotFoundInChain { .. } |
                    BlockchainTreeError::BlockBufferingFailed { .. } => false,
                }
            }
            InsertBlockErrorKind::Internal(_) => {
                // any other error, such as database errors, are considered internal errors
                false
            }
        }
    }

    /// Returns true if this is a block pre merge error.
    pub fn is_block_pre_merge(&self) -> bool {
        matches!(
            self,
            InsertBlockErrorKind::Execution(BlockExecutionError::Validation(
                BlockValidationError::BlockPreMerge { .. }
            ))
        )
    }

    /// Returns true if the error is an execution error
    pub fn is_execution_error(&self) -> bool {
        matches!(self, InsertBlockErrorKind::Execution(_))
    }

    /// Returns true if the error is an internal error
    pub fn is_internal(&self) -> bool {
        matches!(self, InsertBlockErrorKind::Internal(_))
    }

    /// Returns the error if it is a tree error
    pub fn as_tree_error(&self) -> Option<BlockchainTreeError> {
        match self {
            InsertBlockErrorKind::Tree(err) => Some(*err),
            _ => None,
        }
    }

    /// Returns the error if it is a consensus error
    pub fn as_consensus_error(&self) -> Option<&ConsensusError> {
        match self {
            InsertBlockErrorKind::Consensus(err) => Some(err),
            _ => None,
        }
    }

    /// Returns the error if it is an execution error
    pub fn as_execution_error(&self) -> Option<&BlockExecutionError> {
        match self {
            InsertBlockErrorKind::Execution(err) => Some(err),
            _ => None,
        }
    }
}

// This is a convenience impl to convert from crate::Error to InsertBlockErrorKind, most
impl From<crate::Error> for InsertBlockErrorKind {
    fn from(err: crate::Error) -> Self {
        use crate::Error;

        match err {
            Error::Execution(err) => InsertBlockErrorKind::Execution(err),
            Error::Consensus(err) => InsertBlockErrorKind::Consensus(err),
            Error::Database(err) => InsertBlockErrorKind::Internal(Box::new(err)),
            Error::Provider(err) => InsertBlockErrorKind::Internal(Box::new(err)),
            Error::Network(err) => InsertBlockErrorKind::Internal(Box::new(err)),
        }
    }
}
