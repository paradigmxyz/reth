//! Error handling for the blockchain tree

use crate::{consensus::ConsensusError, executor::BlockExecutionError};
use reth_primitives::{BlockHash, BlockNumber, SealedBlock};
use std::fmt::Formatter;

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
}

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

    /// Create a new InsertInvalidBlockError from a tree error
    pub fn tree_error(error: BlockchainTreeError, block: SealedBlock) -> Self {
        Self::new(block, InsertInvalidBlockErrorKind::Tree(error))
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
    pub fn execution_error(error: BlockExecutionError, block: SealedBlock) -> Self {
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
    Execution(BlockExecutionError),
    /// Block violated tree invariants.
    #[error(transparent)]
    Tree(#[from] BlockchainTreeError),
    /// An internal error occurred, like interacting with the database.
    #[error("Internal error")]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl InsertInvalidBlockErrorKind {
    /// Returns true if the error is a tree error
    pub fn is_tree_error(&self) -> bool {
        matches!(self, InsertInvalidBlockErrorKind::Tree(_))
    }

    /// Returns true if the error is a consensus error
    pub fn is_consensus_error(&self) -> bool {
        matches!(self, InsertInvalidBlockErrorKind::Consensus(_))
    }

    /// Returns true if this is a block pre merge error.
    pub fn is_block_pre_merge(&self) -> bool {
        matches!(
            self,
            InsertInvalidBlockErrorKind::Execution(BlockExecutionError::BlockPreMerge { .. })
        )
    }

    /// Returns true if the error is an execution error
    pub fn is_execution_error(&self) -> bool {
        matches!(self, InsertInvalidBlockErrorKind::Execution(_))
    }

    /// Returns the error if it is a tree error
    pub fn as_tree_error(&self) -> Option<BlockchainTreeError> {
        match self {
            InsertInvalidBlockErrorKind::Tree(err) => Some(*err),
            _ => None,
        }
    }

    /// Returns the error if it is a consensus error
    pub fn as_consensus_error(&self) -> Option<&ConsensusError> {
        match self {
            InsertInvalidBlockErrorKind::Consensus(err) => Some(err),
            _ => None,
        }
    }

    /// Returns the error if it is an execution error
    pub fn as_execution_error(&self) -> Option<&BlockExecutionError> {
        match self {
            InsertInvalidBlockErrorKind::Execution(err) => Some(err),
            _ => None,
        }
    }
}

// This is a convenience impl to convert from crate::Error to InsertInvalidBlockErrorKind, most
impl From<crate::Error> for InsertInvalidBlockErrorKind {
    fn from(err: crate::Error) -> Self {
        use crate::Error;

        match err {
            Error::Execution(err) => InsertInvalidBlockErrorKind::Execution(err),
            Error::Consensus(err) => InsertInvalidBlockErrorKind::Consensus(err),
            Error::Database(err) => InsertInvalidBlockErrorKind::Internal(Box::new(err)),
            Error::Provider(err) => InsertInvalidBlockErrorKind::Internal(Box::new(err)),
            Error::Network(err) => InsertInvalidBlockErrorKind::Internal(Box::new(err)),
        }
    }
}
