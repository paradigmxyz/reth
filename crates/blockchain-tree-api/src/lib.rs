//! Interfaces and types for interacting with the blockchain tree.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use self::error::CanonicalError;
use crate::error::InsertBlockError;
use reth_primitives::{
    BlockHash, BlockNumHash, BlockNumber, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::collections::BTreeMap;

pub mod error;

/// * [`BlockchainTreeEngine::insert_block`]: Connect block to chain, execute it and if valid insert
///   block inside tree.
/// * [`BlockchainTreeEngine::finalize_block`]: Remove chains that join to now finalized block, as
///   chain becomes invalid.
/// * [`BlockchainTreeEngine::make_canonical`]: Check if we have the hash of block that we want to
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
    /// with [`BlockValidationKind::Exhaustive`] to validate the state root, if possible to adhere
    /// to the engine API spec.
    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
        validation_kind: BlockValidationKind,
    ) -> Result<InsertPayloadOk, InsertBlockError>;

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    fn finalize_block(&self, finalized_block: BlockNumber) -> ProviderResult<()>;

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
    ) -> Result<(), CanonicalError>;

    /// Update all block hashes. iterate over present and new list of canonical hashes and compare
    /// them. Remove all mismatches, disconnect them, removes all chains and clears all buffered
    /// blocks before the tip.
    fn update_block_hashes_and_clear_buffered(
        &self,
    ) -> Result<BTreeMap<BlockNumber, BlockHash>, CanonicalError>;

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree by attempting to connect the buffered blocks to canonical hashes.
    ///
    /// `N` is the maximum of `max_reorg_depth` and the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    fn connect_buffered_blocks_to_canonical_hashes(&self) -> Result<(), CanonicalError>;

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
}

/// Represents the kind of validation that should be performed when inserting a block.
///
/// The motivation for this is that the engine API spec requires that block's state root is
/// validated when received from the CL.
///
/// This step is very expensive due to how changesets are stored in the database, so we want to
/// avoid doing it if not necessary. Blocks can also originate from the network where this step is
/// not required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockValidationKind {
    /// All validation checks that can be performed.
    ///
    /// This includes validating the state root, if possible.
    ///
    /// Note: This should always be used when inserting blocks that originate from the consensus
    /// layer.
    #[default]
    Exhaustive,
    /// Perform all validation checks except for state root validation.
    SkipStateRootValidation,
}

impl BlockValidationKind {
    /// Returns true if the state root should be validated if possible.
    pub const fn is_exhaustive(&self) -> bool {
        matches!(self, Self::Exhaustive)
    }
}

impl std::fmt::Display for BlockValidationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exhaustive => {
                write!(f, "Exhaustive")
            }
            Self::SkipStateRootValidation => {
                write!(f, "SkipStateRootValidation")
            }
        }
    }
}

/// All possible outcomes of a canonicalization attempt of [`BlockchainTreeEngine::make_canonical`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalOutcome {
    /// The block is already canonical.
    AlreadyCanonical {
        /// Block number and hash of current head.
        head: BlockNumHash,
        /// The corresponding [`SealedHeader`] that was attempted to be made a current head and
        /// is already canonical.
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
    pub const fn header(&self) -> &SealedHeader {
        match self {
            Self::AlreadyCanonical { header, .. } => header,
            Self::Committed { head } => head,
        }
    }

    /// Consumes the outcome and returns the header of the block that was made canonical.
    pub fn into_header(self) -> SealedHeader {
        match self {
            Self::AlreadyCanonical { header, .. } => header,
            Self::Committed { head } => head,
        }
    }

    /// Returns true if the block was already canonical.
    pub const fn is_already_canonical(&self) -> bool {
        matches!(self, Self::AlreadyCanonical { .. })
    }
}

/// Block inclusion can be valid, accepted, or invalid. Invalid blocks are returned as an error
/// variant.
///
/// If we don't know the block's parent, we return `Disconnected`, as we can't claim that the block
/// is valid or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus2 {
    /// The block is valid and block extends canonical chain.
    Valid,
    /// The block may be valid and has an unknown missing ancestor.
    Disconnected {
        /// Current canonical head.
        head: BlockNumHash,
        /// The lowest ancestor block that is not connected to the canonical chain.
        missing_ancestor: BlockNumHash,
    },
}

/// How a payload was inserted if it was valid.
///
/// If the payload was valid, but has already been seen, [`InsertPayloadOk2::AlreadySeen(_)`] is
/// returned, otherwise [`InsertPayloadOk2::Inserted(_)`] is returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertPayloadOk2 {
    /// The payload was valid, but we have already seen it.
    AlreadySeen(BlockStatus2),
    /// The payload was valid and inserted into the tree.
    Inserted(BlockStatus2),
}

/// From Engine API spec, block inclusion can be valid, accepted or invalid.
/// Invalid case is already covered by error, but we need to make distinction
/// between valid blocks that extend canonical chain and the ones that fork off
/// into side chains (see [`BlockAttachment`]). If we don't know the block
/// parent we are returning Disconnected status as we can't make a claim if
/// block is valid or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    /// If block is valid and block extends canonical chain.
    /// In `BlockchainTree` terms, it forks off canonical tip.
    Valid(BlockAttachment),
    /// If block is valid and block forks off canonical chain.
    /// If blocks is not connected to canonical chain.
    Disconnected {
        /// Current canonical head.
        head: BlockNumHash,
        /// The lowest ancestor block that is not connected to the canonical chain.
        missing_ancestor: BlockNumHash,
    },
}

/// Represents what kind of block is being executed and validated.
///
/// This is required to:
/// - differentiate whether trie state updates should be cached.
/// - inform other
///
/// This is required because the state root check can only be performed if the targeted block can be
/// traced back to the canonical __head__.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAttachment {
    /// The `block` is canonical or a descendant of the canonical head.
    /// ([`head..(block.parent)*,block`])
    Canonical,
    /// The block can be traced back to an ancestor of the canonical head: a historical block, but
    /// this chain does __not__ include the canonical head.
    HistoricalFork,
}

impl BlockAttachment {
    /// Returns `true` if the block is canonical or a descendant of the canonical head.
    #[inline]
    pub const fn is_canonical(&self) -> bool {
        matches!(self, Self::Canonical)
    }
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

    /// Returns the _buffered_ (disconnected) header with matching hash from the internal buffer if
    /// it exists.
    ///
    /// Caution: Unlike [`Self::block_by_hash`] this will only return headers that are currently
    /// disconnected from the canonical chain.
    fn buffered_header_by_hash(&self, block_hash: BlockHash) -> Option<SealedHeader>;

    /// Returns true if the tree contains the block with matching hash.
    fn contains(&self, hash: BlockHash) -> bool {
        self.block_by_hash(hash).is_some()
    }

    /// Return whether or not the block is known and in the canonical chain.
    fn is_canonical(&self, hash: BlockHash) -> Result<bool, ProviderError>;

    /// Given the hash of a block, this checks the buffered blocks for the lowest ancestor in the
    /// buffer.
    ///
    /// If there is a buffered block with the given hash, this returns the block itself.
    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders>;

    /// Return `BlockchainTree` best known canonical chain tip (`BlockHash`, `BlockNumber`)
    fn canonical_tip(&self) -> BlockNumHash;

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
    /// [`Self::pending_block`] and [`Self::pending_receipts`] calls.
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
