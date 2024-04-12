//! Standalone types and traits for Reth's blockchain tree.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod engine;
mod viewer;

pub use engine::*;
use reth_primitives::SealedHeader;
use reth_rpc_types::BlockNumHash;
pub use viewer::*;

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
    pub fn is_exhaustive(&self) -> bool {
        matches!(self, BlockValidationKind::Exhaustive)
    }
}

impl std::fmt::Display for BlockValidationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockValidationKind::Exhaustive => {
                write!(f, "Exhaustive")
            }
            BlockValidationKind::SkipStateRootValidation => {
                write!(f, "SkipStateRootValidation")
            }
        }
    }
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
/// between valid blocks that extend canonical chain and the ones that fork off
/// into side chains (see [BlockAttachment]). If we don't know the block
/// parent we are returning Disconnected status as we can't make a claim if
/// block is valid or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    /// If block is valid and block extends canonical chain.
    /// In BlockchainTree terms, it forks off canonical tip.
    Valid(BlockAttachment),
    /// If block is valid and block forks off canonical chain.
    /// If blocks is not connected to canonical chain.
    Disconnected {
        /// The lowest ancestor block that is not connected to the canonical chain.
        missing_ancestor: BlockNumHash,
    },
}

/// Represents what kind of block is being executed and validated.
///
/// This is required to:
/// - differentiate whether trie state updates should be cached.
/// - inform other
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
        matches!(self, BlockAttachment::Canonical)
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
