use std::collections::{BTreeMap, HashSet};

use crate::{executor::Error as ExecutionError, Error};
use async_trait::async_trait;
use reth_primitives::{BlockHash, BlockNumber, SealedBlock, SealedBlockWithSenders};

/// * [BlockchainTree::insert_block]: Connect block to chain, execute it and if valid insert block
///   inside tree.
/// * [BlockchainTree::finalize_block]: Remove chains that join to now finalized block, as chain
///   becomes invalid.
/// * [BlockchainTree::make_canonical]: Check if we have the hash of block that we want to finalize
///   and commit it to db. If we dont have the block, pipeline syncing should start to fetch the
///   blocks from p2p. Do reorg in tables if canonical chain if needed.
#[async_trait]
pub trait BlockchainTreeEngine: BlockchainTreeViewer {
    /// Recover senders and call [`BlockchainTreeEngine::insert_block_with_senders`].
    async fn insert_block(&self, block: SealedBlock) -> Result<BlockStatus, Error> {
        let block = block.seal_with_senders().ok_or(ExecutionError::SenderRecoveryError)?;
        self.insert_block_with_senders(block).await
    }

    /// Insert block with senders
    async fn insert_block_with_senders(
        &self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, Error>;

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    async fn finalize_block(&self, finalized_block: BlockNumber);

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree.
    ///
    /// `N` is the `max_reorg_depth` plus the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTree::finalize_block`]).
    async fn restore_canonical_hashes(
        &self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), Error>;

    /// Make a block and its parent part of the canonical chain.
    ///
    /// # Note
    ///
    /// This unwinds the database if necessary, i.e. if parts of the canonical chain have been
    /// re-orged.
    ///
    /// # Returns
    ///
    /// Returns `Ok` if the blocks were canonicalized, or if the blocks were already canonical.
    async fn make_canonical(&self, block_hash: &BlockHash) -> Result<(), Error>;

    /// Unwind tables and put it inside state
    async fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error>;
}

/// From Engine API spec, block inclusion can be valid, accepted or invalid.
/// Invalid case is already covered by error but we needs to make distinction
/// between if it is valid (extends canonical chain) or just accepted (is side chain).
/// If we dont know the block parent we are returning Disconnected status
/// as we can't make a claim if block is valid or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    /// If block validation is valid and block extends canonical chain.
    /// In BlockchainTree sense it forks on canonical tip.
    Valid,
    /// If the block is valid, but it does not extend canonical chain
    /// (It is side chain) or hasn't been fully validated but ancestors of a payload are known.
    Accepted,
    /// If blocks is not connected to canonical chain.
    Disconnected,
}

/// Allows read only functionality on the blockchain tree.
#[async_trait]
pub trait BlockchainTreeViewer {
    /// Get all pending block numbers and their hashes.
    async fn pending_blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>>;

    /// Canonical block number and hashes best known by the tree.
    async fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash>;
}
