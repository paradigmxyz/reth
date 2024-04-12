use reth_interfaces::{
    blockchain_tree::{CanonicalError, InsertBlockError},
    RethResult,
};
use reth_primitives::{BlockHash, BlockNumber, SealedBlock, SealedBlockWithSenders};

use crate::{BlockValidationKind, BlockchainTreeViewer, CanonicalOutcome, InsertPayloadOk};

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
        validation_kind: BlockValidationKind,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block, validation_kind),
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

    /// Inserts block with senders
    ///
    /// The `validation_kind` parameter controls which validation checks are performed.
    ///
    /// Caution: If the block was received from the consensus layer, this should always be called
    /// with [BlockValidationKind::Exhaustive] to validate the state root, if possible to adhere to
    /// the engine API spec.
    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
        validation_kind: BlockValidationKind,
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
    fn make_canonical(&self, block_hash: BlockHash) -> Result<CanonicalOutcome, CanonicalError>;

    /// Unwind tables and put it inside state
    fn unwind(&self, unwind_to: BlockNumber) -> RethResult<()>;
}
