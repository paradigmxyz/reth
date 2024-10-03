//! Error handling for the blockchain tree

use alloy_primitives::{BlockHash, BlockNumber};
use reth_consensus::ConsensusError;
use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_primitives::SealedBlock;
pub use reth_storage_errors::provider::ProviderError;

/// Various error cases that can occur when a block violates tree assumptions.
#[derive(Debug, Clone, Copy, thiserror::Error, Eq, PartialEq)]
pub enum BlockchainTreeError {
    /// Thrown if the block number is lower than the last finalized block number.
    #[error("block number is lower than the last finalized block number #{last_finalized}")]
    PendingBlockIsFinalized {
        /// The block number of the last finalized block.
        last_finalized: BlockNumber,
    },
    /// Thrown if no side chain could be found for the block.
    #[error("chainId can't be found in BlockchainTree with internal index {chain_id}")]
    BlockSideChainIdConsistency {
        /// The internal identifier for the side chain.
        chain_id: u64,
    },
    /// Thrown if a canonical chain header cannot be found.
    #[error("canonical chain header {block_hash} can't be found")]
    CanonicalChain {
        /// The block hash of the missing canonical chain header.
        block_hash: BlockHash,
    },
    /// Thrown if a block number cannot be found in the blockchain tree chain.
    #[error("block number #{block_number} not found in blockchain tree chain")]
    BlockNumberNotFoundInChain {
        /// The block number that could not be found.
        block_number: BlockNumber,
    },
    /// Thrown if a block hash cannot be found in the blockchain tree chain.
    #[error("block hash {block_hash} not found in blockchain tree chain")]
    BlockHashNotFoundInChain {
        /// The block hash that could not be found.
        block_hash: BlockHash,
    },
    /// Thrown if the block failed to buffer
    #[error("block with hash {block_hash} failed to buffer")]
    BlockBufferingFailed {
        /// The block hash of the block that failed to buffer.
        block_hash: BlockHash,
    },
    /// Thrown when trying to access genesis parent.
    #[error("genesis block has no parent")]
    GenesisBlockHasNoParent,
}

/// Canonical Errors
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum CanonicalError {
    /// Error originating from validation operations.
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    /// Error originating from blockchain tree operations.
    #[error(transparent)]
    BlockchainTree(#[from] BlockchainTreeError),
    /// Error originating from a provider operation.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Error indicating a transaction reverted during execution.
    #[error("transaction error on revert: {0}")]
    CanonicalRevert(String),
    /// Error indicating a transaction failed to commit during execution.
    #[error("transaction error on commit: {0}")]
    CanonicalCommit(String),
    /// Error indicating that a previous optimistic sync target was re-orged
    #[error("transaction error on revert: {0}")]
    OptimisticTargetRevert(BlockNumber),
}

impl CanonicalError {
    /// Returns `true` if the error is fatal.
    pub const fn is_fatal(&self) -> bool {
        matches!(self, Self::CanonicalCommit(_) | Self::CanonicalRevert(_))
    }

    /// Returns `true` if the underlying error matches
    /// [`BlockchainTreeError::BlockHashNotFoundInChain`].
    pub const fn is_block_hash_not_found(&self) -> bool {
        matches!(self, Self::BlockchainTree(BlockchainTreeError::BlockHashNotFoundInChain { .. }))
    }

    /// Returns `Some(BlockNumber)` if the underlying error matches
    /// [`CanonicalError::OptimisticTargetRevert`].
    pub const fn optimistic_revert_block_number(&self) -> Option<BlockNumber> {
        match self {
            Self::OptimisticTargetRevert(block_number) => Some(*block_number),
            _ => None,
        }
    }
}

/// Error thrown when inserting a block failed because the block is considered invalid.
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct InsertBlockError {
    inner: Box<InsertBlockErrorData>,
}

// === impl InsertBlockError ===

impl InsertBlockError {
    /// Create a new `InsertInvalidBlockError`
    pub fn new(block: SealedBlock, kind: InsertBlockErrorKind) -> Self {
        Self { inner: InsertBlockErrorData::boxed(block, kind) }
    }

    /// Create a new `InsertInvalidBlockError` from a tree error
    pub fn tree_error(error: BlockchainTreeError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::Tree(error))
    }

    /// Create a new `InsertInvalidBlockError` from a consensus error
    pub fn consensus_error(error: ConsensusError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::Consensus(error))
    }

    /// Create a new `InsertInvalidBlockError` from a consensus error
    pub fn sender_recovery_error(block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKind::SenderRecovery)
    }

    /// Create a new `InsertInvalidBlockError` from an execution error
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
    pub const fn kind(&self) -> &InsertBlockErrorKind {
        &self.inner.kind
    }

    /// Returns the block that resulted in the error
    #[inline]
    pub const fn block(&self) -> &SealedBlock {
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
            "Failed to insert block (hash={}, number={}, parent_hash={}): {}",
            self.block.hash(),
            self.block.number,
            self.block.parent_hash,
            self.kind
        )
    }
}

impl std::fmt::Debug for InsertBlockErrorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertBlockError")
            .field("error", &self.kind)
            .field("hash", &self.block.hash())
            .field("number", &self.block.number)
            .field("parent_hash", &self.block.parent_hash)
            .field("num_txs", &self.block.body.transactions.len())
            .finish_non_exhaustive()
    }
}

impl core::error::Error for InsertBlockErrorData {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.kind)
    }
}

impl InsertBlockErrorData {
    const fn new(block: SealedBlock, kind: InsertBlockErrorKind) -> Self {
        Self { block, kind }
    }

    fn boxed(block: SealedBlock, kind: InsertBlockErrorKind) -> Box<Self> {
        Box::new(Self::new(block, kind))
    }
}

struct InsertBlockErrorDataTwo {
    block: SealedBlock,
    kind: InsertBlockErrorKindTwo,
}

impl std::fmt::Display for InsertBlockErrorDataTwo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Failed to insert block (hash={}, number={}, parent_hash={}): {}",
            self.block.hash(),
            self.block.number,
            self.block.parent_hash,
            self.kind
        )
    }
}

impl std::fmt::Debug for InsertBlockErrorDataTwo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertBlockError")
            .field("error", &self.kind)
            .field("hash", &self.block.hash())
            .field("number", &self.block.number)
            .field("parent_hash", &self.block.parent_hash)
            .field("num_txs", &self.block.body.transactions.len())
            .finish_non_exhaustive()
    }
}

impl core::error::Error for InsertBlockErrorDataTwo {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.kind)
    }
}

impl InsertBlockErrorDataTwo {
    const fn new(block: SealedBlock, kind: InsertBlockErrorKindTwo) -> Self {
        Self { block, kind }
    }

    fn boxed(block: SealedBlock, kind: InsertBlockErrorKindTwo) -> Box<Self> {
        Box::new(Self::new(block, kind))
    }
}

/// Error thrown when inserting a block failed because the block is considered invalid.
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct InsertBlockErrorTwo {
    inner: Box<InsertBlockErrorDataTwo>,
}

// === impl InsertBlockErrorTwo ===

impl InsertBlockErrorTwo {
    /// Create a new `InsertInvalidBlockErrorTwo`
    pub fn new(block: SealedBlock, kind: InsertBlockErrorKindTwo) -> Self {
        Self { inner: InsertBlockErrorDataTwo::boxed(block, kind) }
    }

    /// Create a new `InsertInvalidBlockError` from a consensus error
    pub fn consensus_error(error: ConsensusError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKindTwo::Consensus(error))
    }

    /// Create a new `InsertInvalidBlockError` from a consensus error
    pub fn sender_recovery_error(block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKindTwo::SenderRecovery)
    }

    /// Create a new `InsertInvalidBlockError` from an execution error
    pub fn execution_error(error: BlockExecutionError, block: SealedBlock) -> Self {
        Self::new(block, InsertBlockErrorKindTwo::Execution(error))
    }

    /// Consumes the error and returns the block that resulted in the error
    #[inline]
    pub fn into_block(self) -> SealedBlock {
        self.inner.block
    }

    /// Returns the error kind
    #[inline]
    pub const fn kind(&self) -> &InsertBlockErrorKindTwo {
        &self.inner.kind
    }

    /// Returns the block that resulted in the error
    #[inline]
    pub const fn block(&self) -> &SealedBlock {
        &self.inner.block
    }

    /// Consumes the type and returns the block and error kind.
    #[inline]
    pub fn split(self) -> (SealedBlock, InsertBlockErrorKindTwo) {
        let inner = *self.inner;
        (inner.block, inner.kind)
    }
}

impl std::fmt::Debug for InsertBlockErrorTwo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.inner, f)
    }
}

/// All error variants possible when inserting a block
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockErrorKindTwo {
    /// Failed to recover senders for the block
    #[error("failed to recover senders for block")]
    SenderRecovery,
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Block execution failed.
    #[error(transparent)]
    Execution(#[from] BlockExecutionError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Other errors.
    #[error(transparent)]
    Other(#[from] Box<dyn core::error::Error + Send + Sync + 'static>),
}

impl InsertBlockErrorKindTwo {
    /// Returns an [`InsertBlockValidationError`] if the error is caused by an invalid block.
    ///
    /// Returns an [`InsertBlockFatalError`] if the error is caused by an error that is not
    /// validation related or is otherwise fatal.
    ///
    /// This is intended to be used to determine if we should respond `INVALID` as a response when
    /// processing a new block.
    pub fn ensure_validation_error(
        self,
    ) -> Result<InsertBlockValidationError, InsertBlockFatalError> {
        match self {
            Self::SenderRecovery => Ok(InsertBlockValidationError::SenderRecovery),
            Self::Consensus(err) => Ok(InsertBlockValidationError::Consensus(err)),
            // other execution errors that are considered internal errors
            Self::Execution(err) => {
                match err {
                    BlockExecutionError::Validation(err) => {
                        Ok(InsertBlockValidationError::Validation(err))
                    }
                    BlockExecutionError::Consensus(err) => {
                        Ok(InsertBlockValidationError::Consensus(err))
                    }
                    // these are internal errors, not caused by an invalid block
                    BlockExecutionError::Internal(error) => {
                        Err(InsertBlockFatalError::BlockExecutionError(error))
                    }
                }
            }
            Self::Provider(err) => Err(InsertBlockFatalError::Provider(err)),
            Self::Other(err) => Err(InternalBlockExecutionError::Other(err).into()),
        }
    }
}

/// Error variants that are not caused by invalid blocks
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockFatalError {
    /// A provider error
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// An internal / fatal block execution error
    #[error(transparent)]
    BlockExecutionError(#[from] InternalBlockExecutionError),
}

/// Error variants that are caused by invalid blocks
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockValidationError {
    /// Failed to recover senders for the block
    #[error("failed to recover senders for block")]
    SenderRecovery,
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Validation error, transparently wrapping [`BlockValidationError`]
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
}

impl InsertBlockValidationError {
    /// Returns true if this is a block pre merge error.
    pub const fn is_block_pre_merge(&self) -> bool {
        matches!(self, Self::Validation(BlockValidationError::BlockPreMerge { .. }))
    }
}

/// All error variants possible when inserting a block
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockErrorKind {
    /// Failed to recover senders for the block
    #[error("failed to recover senders for block")]
    SenderRecovery,
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Block execution failed.
    #[error(transparent)]
    Execution(#[from] BlockExecutionError),
    /// Block violated tree invariants.
    #[error(transparent)]
    Tree(#[from] BlockchainTreeError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// An internal error occurred, like interacting with the database.
    #[error(transparent)]
    Internal(#[from] Box<dyn core::error::Error + Send + Sync>),
    /// Canonical error.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

impl InsertBlockErrorKind {
    /// Returns true if the error is a tree error
    pub const fn is_tree_error(&self) -> bool {
        matches!(self, Self::Tree(_))
    }

    /// Returns true if the error is a consensus error
    pub const fn is_consensus_error(&self) -> bool {
        matches!(self, Self::Consensus(_))
    }

    /// Returns true if this error is a state root error
    pub const fn is_state_root_error(&self) -> bool {
        // we need to get the state root errors inside of the different variant branches
        match self {
            Self::Execution(err) => {
                matches!(
                    err,
                    BlockExecutionError::Validation(BlockValidationError::StateRoot { .. })
                )
            }
            Self::Canonical(err) => {
                matches!(
                    err,
                    CanonicalError::Validation(BlockValidationError::StateRoot { .. }) |
                        CanonicalError::Provider(
                            ProviderError::StateRootMismatch(_) |
                                ProviderError::UnwindStateRootMismatch(_)
                        )
                )
            }
            Self::Provider(err) => {
                matches!(
                    err,
                    ProviderError::StateRootMismatch(_) | ProviderError::UnwindStateRootMismatch(_)
                )
            }
            _ => false,
        }
    }

    /// Returns true if the error is caused by an invalid block
    ///
    /// This is intended to be used to determine if the block should be marked as invalid.
    #[allow(clippy::match_same_arms)]
    pub const fn is_invalid_block(&self) -> bool {
        match self {
            Self::SenderRecovery | Self::Consensus(_) => true,
            // other execution errors that are considered internal errors
            Self::Execution(err) => {
                match err {
                    BlockExecutionError::Validation(_) | BlockExecutionError::Consensus(_) => {
                        // this is caused by an invalid block
                        true
                    }
                    // these are internal errors, not caused by an invalid block
                    BlockExecutionError::Internal(_) => false,
                }
            }
            Self::Tree(err) => {
                match err {
                    BlockchainTreeError::PendingBlockIsFinalized { .. } => {
                        // the block's number is lower than the finalized block's number
                        true
                    }
                    BlockchainTreeError::BlockSideChainIdConsistency { .. } |
                    BlockchainTreeError::CanonicalChain { .. } |
                    BlockchainTreeError::BlockNumberNotFoundInChain { .. } |
                    BlockchainTreeError::BlockHashNotFoundInChain { .. } |
                    BlockchainTreeError::BlockBufferingFailed { .. } |
                    BlockchainTreeError::GenesisBlockHasNoParent => false,
                }
            }
            Self::Provider(_) | Self::Internal(_) => {
                // any other error, such as database errors, are considered internal errors
                false
            }
            Self::Canonical(err) => match err {
                CanonicalError::BlockchainTree(_) |
                CanonicalError::CanonicalCommit(_) |
                CanonicalError::CanonicalRevert(_) |
                CanonicalError::OptimisticTargetRevert(_) |
                CanonicalError::Provider(_) => false,
                CanonicalError::Validation(_) => true,
            },
        }
    }

    /// Returns true if this is a block pre merge error.
    pub const fn is_block_pre_merge(&self) -> bool {
        matches!(
            self,
            Self::Execution(BlockExecutionError::Validation(
                BlockValidationError::BlockPreMerge { .. }
            ))
        )
    }

    /// Returns true if the error is an execution error
    pub const fn is_execution_error(&self) -> bool {
        matches!(self, Self::Execution(_))
    }

    /// Returns true if the error is an internal error
    pub const fn is_internal(&self) -> bool {
        matches!(self, Self::Internal(_))
    }

    /// Returns the error if it is a tree error
    pub const fn as_tree_error(&self) -> Option<BlockchainTreeError> {
        match self {
            Self::Tree(err) => Some(*err),
            _ => None,
        }
    }

    /// Returns the error if it is a consensus error
    pub const fn as_consensus_error(&self) -> Option<&ConsensusError> {
        match self {
            Self::Consensus(err) => Some(err),
            _ => None,
        }
    }

    /// Returns the error if it is an execution error
    pub const fn as_execution_error(&self) -> Option<&BlockExecutionError> {
        match self {
            Self::Execution(err) => Some(err),
            _ => None,
        }
    }
}
