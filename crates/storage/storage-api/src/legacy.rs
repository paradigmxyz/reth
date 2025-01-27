//! Traits used by the legacy execution engine.
//!
//! This module is scheduled for removal in the future.

use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber};
use auto_impl::auto_impl;
use reth_execution_types::ExecutionOutcome;
use reth_storage_errors::provider::{ProviderError, ProviderResult};

/// Blockchain trait provider that gives access to the blockchain state that is not yet committed
/// (pending).
pub trait BlockchainTreePendingStateProvider: Send + Sync {
    /// Returns a state provider that includes all state changes of the given (pending) block hash.
    ///
    /// In other words, the state provider will return the state after all transactions of the given
    /// hash have been executed.
    fn pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> ProviderResult<Box<dyn FullExecutionDataProvider>> {
        self.find_pending_state_provider(block_hash)
            .ok_or(ProviderError::StateForHashNotFound(block_hash))
    }

    /// Returns state provider if a matching block exists.
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn FullExecutionDataProvider>>;
}

/// Provides data required for post-block execution.
///
/// This trait offers methods to access essential post-execution data, including the state changes
/// in accounts and storage, as well as block hashes for both the pending and canonical chains.
///
/// The trait includes:
/// * [`ExecutionOutcome`] - Captures all account and storage changes in the pending chain.
/// * Block hashes - Provides access to the block hashes of both the pending chain and canonical
///   blocks.
#[auto_impl(&, Box)]
pub trait ExecutionDataProvider: Send + Sync {
    /// Return the execution outcome.
    fn execution_outcome(&self) -> &ExecutionOutcome;
    /// Return block hash by block number of pending or canonical chain.
    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash>;
}

impl ExecutionDataProvider for ExecutionOutcome {
    fn execution_outcome(&self) -> &ExecutionOutcome {
        self
    }

    /// Always returns [None] because we don't have any information about the block header.
    fn block_hash(&self, _block_number: BlockNumber) -> Option<BlockHash> {
        None
    }
}

/// Fork data needed for execution on it.
///
/// It contains a canonical fork, the block on what pending chain was forked from.
#[auto_impl(&, Box)]
pub trait BlockExecutionForkProvider {
    /// Return canonical fork, the block on what post state was forked from.
    ///
    /// Needed to create state provider.
    fn canonical_fork(&self) -> BlockNumHash;
}

/// Provides comprehensive post-execution state data required for further execution.
///
/// This trait is used to create a state provider over the pending state and is a combination of
/// [`ExecutionDataProvider`] and [`BlockExecutionForkProvider`].
///
/// The pending state includes:
/// * `ExecutionOutcome`: Contains all changes to accounts and storage within the pending chain.
/// * Block hashes: Represents hashes of both the pending chain and canonical blocks.
/// * Canonical fork: Denotes the block from which the pending chain forked.
pub trait FullExecutionDataProvider: ExecutionDataProvider + BlockExecutionForkProvider {}

impl<T> FullExecutionDataProvider for T where T: ExecutionDataProvider + BlockExecutionForkProvider {}
