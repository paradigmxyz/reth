use alloy_primitives::B256;
use std::fmt;

/// Error returned when an overlay cannot be built from in-memory blocks.
///
/// Where this crate returns a [`ProviderError`], the overlay error is its
/// [`ProviderError::Other`] variant, and [`ProviderError::downcast_other_ref`] returns it.
///
/// [`ProviderError`]: reth_errors::ProviderError
/// [`ProviderError::Other`]: reth_errors::ProviderError::Other
/// [`ProviderError::downcast_other_ref`]: reth_errors::ProviderError::downcast_other_ref
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayError {
    /// The in-memory state does not track the requested tip.
    ///
    /// This is expected in normal operation: the block was removed, as persisted or as pruned,
    /// after the tip was chosen.
    TipNotInMemory {
        /// Hash of the requested tip.
        tip_hash: B256,
    },
    /// The in-memory chain that ends at the tip does not lead to the requested anchor.
    ///
    /// This is expected in normal operation if the anchor was chosen before the chain of the tip
    /// was read: canonical blocks removed as persisted in between end the chain above the
    /// anchor. Otherwise it indicates an inconsistency: the anchor is not an ancestor of the tip,
    /// or the parent links of the tracked blocks are not the chain of the tip.
    AnchorNotReached {
        /// Hash of the requested tip.
        tip_hash: B256,
        /// Hash of the requested anchor.
        anchor_hash: B256,
    },
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TipNotInMemory { tip_hash } => {
                write!(f, "overlay tip {tip_hash} is not an in-memory block")
            }
            Self::AnchorNotReached { tip_hash, anchor_hash } => write!(
                f,
                "in-memory chain ending at overlay tip {tip_hash} does not lead to anchor {anchor_hash}"
            ),
        }
    }
}

impl std::error::Error for OverlayError {}
