use std::collections::{BTreeMap, HashSet};

use reth_interfaces::provider::ProviderError;
use reth_primitives::{
    BlockHash, BlockNumber, Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader,
};
use reth_rpc_types::BlockNumHash;

/// Allows read only functionality on the blockchain tree.
///
/// Tree contains all blocks that are not canonical that can potentially be included
/// as canonical chain. For better explanation we can group blocks into four groups:
/// * Canonical chain blocks
/// * Side chain blocks. Side chain are block that forks from canonical chain but not its tip.
/// * Pending blocks that extend the canonical chain but are not yet included.
/// * Future pending blocks that extend the pending blocks.
pub trait BlockchainTreeViewer: Send + Sync {
    /// Returns both pending and side-chain block numbers and their hashes.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>>;

    /// Returns the header with matching hash from the tree, if it exists.
    ///
    /// Caution: This will not return headers from the canonical chain.
    fn header_by_hash(&self, hash: BlockHash) -> Option<SealedHeader>;

    /// Returns the block with matching hash from the tree, if it exists.
    ///
    /// Caution: This will not return blocks from the canonical chain or buffered blocks that are
    /// disconnected from the canonical chain.
    fn block_by_hash(&self, hash: BlockHash) -> Option<SealedBlock>;

    /// Returns the block with matching hash from the tree, if it exists.
    ///
    /// Caution: This will not return blocks from the canonical chain or buffered blocks that are
    /// disconnected from the canonical chain.
    fn block_with_senders_by_hash(&self, hash: BlockHash) -> Option<SealedBlockWithSenders>;

    /// Returns the _buffered_ (disconnected) block with matching hash from the internal buffer if
    /// it exists.
    ///
    /// Caution: Unlike [Self::block_by_hash] this will only return blocks that are currently
    /// disconnected from the canonical chain.
    fn buffered_block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock>;

    /// Returns the _buffered_ (disconnected) header with matching hash from the internal buffer if
    /// it exists.
    ///
    /// Caution: Unlike [Self::block_by_hash] this will only return headers that are currently
    /// disconnected from the canonical chain.
    fn buffered_header_by_hash(&self, block_hash: BlockHash) -> Option<SealedHeader>;

    /// Returns true if the tree contains the block with matching hash.
    fn contains(&self, hash: BlockHash) -> bool {
        self.block_by_hash(hash).is_some()
    }

    /// Canonical block number and hashes best known by the tree.
    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash>;

    /// Given the parent hash of a block, this tries to find the last ancestor that is part of the
    /// canonical chain.
    ///
    /// In other words, this will walk up the (side) chain starting with the given hash and return
    /// the first block that's canonical.
    ///
    /// Note: this could be the given `parent_hash` if it's already canonical.
    fn find_canonical_ancestor(&self, parent_hash: BlockHash) -> Option<BlockHash>;

    /// Return whether or not the block is known and in the canonical chain.
    fn is_canonical(&self, hash: BlockHash) -> Result<bool, ProviderError>;

    /// Given the hash of a block, this checks the buffered blocks for the lowest ancestor in the
    /// buffer.
    ///
    /// If there is a buffered block with the given hash, this returns the block itself.
    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders>;

    /// Return BlockchainTree best known canonical chain tip (BlockHash, BlockNumber)
    fn canonical_tip(&self) -> BlockNumHash;

    /// Return block hashes that extends the canonical chain tip by one.
    /// This is used to fetch what is considered the pending blocks, blocks that
    /// has best chance to become canonical.
    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>);

    /// Return block number and hash that extends the canonical chain tip by one.
    ///
    /// If there is no such block, this returns `None`.
    fn pending_block_num_hash(&self) -> Option<BlockNumHash>;

    /// Returns the pending block if there is one.
    fn pending_block(&self) -> Option<SealedBlock> {
        self.block_by_hash(self.pending_block_num_hash()?.hash)
    }

    /// Returns the pending block if there is one.
    fn pending_block_with_senders(&self) -> Option<SealedBlockWithSenders> {
        self.block_with_senders_by_hash(self.pending_block_num_hash()?.hash)
    }

    /// Returns the pending block and its receipts in one call.
    ///
    /// This exists to prevent a potential data race if the pending block changes in between
    /// [Self::pending_block] and [Self::pending_receipts] calls.
    fn pending_block_and_receipts(&self) -> Option<(SealedBlock, Vec<Receipt>)>;

    /// Returns the pending receipts if there is one.
    fn pending_receipts(&self) -> Option<Vec<Receipt>> {
        self.receipts_by_block_hash(self.pending_block_num_hash()?.hash)
    }

    /// Returns the pending receipts if there is one.
    fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<Receipt>>;

    /// Returns the pending block if there is one.
    fn pending_header(&self) -> Option<SealedHeader> {
        self.header_by_hash(self.pending_block_num_hash()?.hash)
    }
}
