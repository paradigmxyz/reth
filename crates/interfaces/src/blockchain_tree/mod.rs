use crate::{blockchain_tree::error::InsertBlockError, RethResult};
use reth_primitives::{
    BlockHash, BlockNumHash, BlockNumber, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader,
};
use std::collections::{BTreeMap, HashSet};

pub mod error;

/// * [BlockchainTreeEngine::insert_block]: Connect block to chain, execute it and if valid insert
///   block inside tree.
/// * [BlockchainTreeEngine::finalize_block]: Remove chains that join to now finalized block, as
///   chain becomes invalid.
/// * [BlockchainTreeEngine::make_canonical]: Check if we have the hash of block that we want to
///   finalize and commit it to db. If we don't have the block, syncing should start to fetch the
///   blocks from p2p. Do reorg in tables if canonical chain if needed.
pub trait BlockchainTreeEngine: BlockchainTreeViewer + Send + Sync {
    /// Recover senders and call [`BlockchainTreeEngine::insert_block`].
    ///
    /// This will recover all senders of the transactions in the block first, and then try to insert
    /// the block.
    fn insert_block_without_senders(
        &self,
        block: SealedBlock,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    /// Recover senders and call [`BlockchainTreeEngine::buffer_block`].
    ///
    /// This will recover all senders of the transactions in the block first, and then try to buffer
    /// the block.
    fn buffer_block_without_senders(&self, block: SealedBlock) -> Result<(), InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.buffer_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    /// Buffer block with senders
    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError>;

    /// Insert block with senders
    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
    ) -> Result<InsertPayloadOk, InsertBlockError>;

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    fn finalize_block(&self, finalized_block: BlockNumber);

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree by attempting to connect the buffered blocks to canonical hashes.
    ///
    ///
    /// `N` is the maximum of `max_reorg_depth` and the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTreeEngine::finalize_block`]).
    fn connect_buffered_blocks_to_canonical_hashes_and_finalize(
        &self,
        last_finalized_block: BlockNumber,
    ) -> RethResult<()>;

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree by attempting to connect the buffered blocks to canonical hashes.
    ///
    /// `N` is the maximum of `max_reorg_depth` and the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    fn connect_buffered_blocks_to_canonical_hashes(&self) -> RethResult<()>;

    /// Make a block and its parent chain part of the canonical chain by committing it to the
    /// database.
    ///
    /// # Note
    ///
    /// This unwinds the database if necessary, i.e. if parts of the canonical chain have been
    /// re-orged.
    ///
    /// # Returns
    ///
    /// Returns `Ok` if the blocks were canonicalized, or if the blocks were already canonical.
    fn make_canonical(&self, block_hash: &BlockHash) -> RethResult<CanonicalOutcome>;

    /// Unwind tables and put it inside state
    fn unwind(&self, unwind_to: BlockNumber) -> RethResult<()>;
}

/// All possible outcomes of a canonicalization attempt of [BlockchainTreeEngine::make_canonical].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalOutcome {
    /// The block is already canonical.
    AlreadyCanonical {
        /// The corresponding [SealedHeader] that is already canonical.
        header: SealedHeader,
    },
    /// Committed the block to the database.
    Committed {
        /// The new corresponding canonical head
        head: SealedHeader,
    },
}

impl CanonicalOutcome {
    /// Returns the header of the block that was made canonical.
    pub fn header(&self) -> &SealedHeader {
        match self {
            CanonicalOutcome::AlreadyCanonical { header } => header,
            CanonicalOutcome::Committed { head } => head,
        }
    }

    /// Consumes the outcome and returns the header of the block that was made canonical.
    pub fn into_header(self) -> SealedHeader {
        match self {
            CanonicalOutcome::AlreadyCanonical { header } => header,
            CanonicalOutcome::Committed { head } => head,
        }
    }

    /// Returns true if the block was already canonical.
    pub fn is_already_canonical(&self) -> bool {
        matches!(self, CanonicalOutcome::AlreadyCanonical { .. })
    }
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
    /// If the block is valid, but it does not extend canonical chain.
    /// (It is side chain) or hasn't been fully validated but ancestors of a payload are known.
    Accepted,
    /// If blocks is not connected to canonical chain.
    Disconnected {
        /// The lowest ancestor block that is not connected to the canonical chain.
        missing_ancestor: BlockNumHash,
    },
}

/// How a payload was inserted if it was valid.
///
/// If the payload was valid, but has already been seen, [`InsertPayloadOk::AlreadySeen(_)`] is
/// returned, otherwise [`InsertPayloadOk::Inserted(_)`] is returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertPayloadOk {
    /// The payload was valid, but we have already seen it.
    AlreadySeen(BlockStatus),
    /// The payload was valid and inserted into the tree.
    Inserted(BlockStatus),
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
    fn is_canonical(&self, hash: BlockHash) -> RethResult<bool>;

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
