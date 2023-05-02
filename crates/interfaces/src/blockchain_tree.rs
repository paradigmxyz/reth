use crate::{executor::Error as ExecutionError, Error};
use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, SealedBlock, SealedBlockWithSenders};
use std::collections::{BTreeMap, HashSet};

/// * [BlockchainTreeEngine::insert_block]: Connect block to chain, execute it and if valid insert
///   block inside tree.
/// * [BlockchainTreeEngine::finalize_block]: Remove chains that join to now finalized block, as
///   chain becomes invalid.
/// * [BlockchainTreeEngine::make_canonical]: Check if we have the hash of block that we want to
///   finalize and commit it to db. If we dont have the block, pipeline syncing should start to
///   fetch the blocks from p2p. Do reorg in tables if canonical chain if needed.
pub trait BlockchainTreeEngine: BlockchainTreeViewer + Send + Sync {
    /// Recover senders and call [`BlockchainTreeEngine::insert_block`].
    fn insert_block_without_senders(&self, block: SealedBlock) -> Result<BlockStatus, Error> {
        let block = block.seal_with_senders().ok_or(ExecutionError::SenderRecoveryError)?;
        self.insert_block(block)
    }

    /// Recover senders and call [`BlockchainTreeEngine::buffer_block`].
    fn buffer_block_without_sender(&self, block: SealedBlock) -> Result<(), Error> {
        let block = block.seal_with_senders().ok_or(ExecutionError::SenderRecoveryError)?;
        self.buffer_block(block)
    }

    /// buffer block with senders
    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), Error>;

    /// Insert block with senders
    fn insert_block(&self, block: SealedBlockWithSenders) -> Result<BlockStatus, Error>;

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    fn finalize_block(&self, finalized_block: BlockNumber);

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree.
    ///
    /// `N` is the `max_reorg_depth` plus the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTreeEngine::finalize_block`]).
    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<(), Error>;

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
    fn make_canonical(&self, block_hash: &BlockHash) -> Result<(), Error>;

    /// Unwind tables and put it inside state
    fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error>;
}

/// From Engine API spec, block inclusion can be valid, accepted or invalid.
/// Invalid case is already covered by error, but we need to make distinction
/// between if it is valid (extends canonical chain) or just accepted (is side chain).
/// If we don't know the block parent we are returning Disconnected status
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
///
/// Tree contains all blocks that are not canonical that can potentially be included
/// as canonical chain. For better explanation we can group blocks into four groups:
/// * Canonical chain blocks
/// * Side chain blocks. Side chain are block that forks from canonical chain but not its tip.
/// * Pending blocks that extend the canonical chain but are not yet included.
/// * Future pending blocks that extend the pending blocks.
pub trait BlockchainTreeViewer: Send + Sync {
    /// Returns both pending and sidechain block numbers and their hashes.
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>>;

    /// Returns the block with matching hash.
    fn block_by_hash(&self, hash: BlockHash) -> Option<SealedBlock>;

    /// Canonical block number and hashes best known by the tree.
    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash>;

    /// Given a hash, this tries to find the last ancestor that is part of the canonical chain.
    ///
    /// In other words, this will walk up the chain starting with the given hash and return the
    /// first block that's canonical.
    fn find_canonical_ancestor(&self, hash: BlockHash) -> Option<BlockHash>;

    /// Return BlockchainTree best known canonical chain tip (BlockHash, BlockNumber)
    fn canonical_tip(&self) -> BlockNumHash;

    /// Return block hashes that extends the canonical chain tip by one.
    /// This is used to fetch what is considered the pending blocks, blocks that
    /// has best chance to become canonical.
    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>);

    /// Return block hashes that extends the canonical chain tip by one.
    ///
    /// If there is no such block, return `None`.
    fn pending_block_num_hash(&self) -> Option<BlockNumHash>;

    /// Returns the pending block if there is one.
    fn pending_block(&self) -> Option<SealedBlock> {
        self.block_by_hash(self.pending_block_num_hash()?.hash)
    }
}
