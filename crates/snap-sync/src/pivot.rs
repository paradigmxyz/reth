//! Chooses the canonical block a snap generation is anchored to.
//!
//! [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189#synchronization-algorithm) pivot selection
//! anchors synchronization at a block "sufficiently behind the chain head [...] to reduce the
//! likelihood of P being reorged while remaining recent enough that serving peers still hold its
//! state in memory". Those two pressures are what this policy balances: too close to the head and
//! the anchor is reorged, too far and no peer will serve its state.
//!
//! Two choices depart from the EIP's example: a finalized block is preferred as the anchor when one
//! is available, and re-anchoring starts once a pivot lags by 96 blocks rather than at the edge of
//! the window peers still serve state for.

use crate::{SnapGeneration, SnapPhase, SnapSyncError};
use alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS;
use reth_primitives_traits::AlloyBlockHeader;
use reth_storage_api::HeaderProvider;

// EIP-8189's example anchor, matching go-ethereum's `fsMinFullBlocks`.
const DEFAULT_HEAD_DISTANCE: u64 = 64;

// Blocks of state history a serving peer is assumed to still hold, mirroring reth's own
// `SNAPSHOT_STATE_RETENTION`.
const SERVED_STATE_WINDOW: u64 = 128;

// Re-anchor before the pivot reaches the edge of that window, so ranges in flight do not fail
// against a root peers just dropped.
const DEFAULT_ADVANCE_AFTER: u64 = SERVED_STATE_WINDOW - DEFAULT_HEAD_DISTANCE / 2;

/// Distance and history bounds that decide where a generation is anchored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapPivotPolicy {
    // Blocks behind the head to anchor at when no finalized block is available.
    head_distance: u64,
    // Pivot lag that triggers re-anchoring while ranges are still downloading.
    advance_after: u64,
    // Blocks of block access list history a peer is assumed to still serve.
    history: u64,
}

impl Default for SnapPivotPolicy {
    fn default() -> Self {
        Self {
            head_distance: DEFAULT_HEAD_DISTANCE,
            advance_after: DEFAULT_ADVANCE_AFTER,
            history: BAL_RETENTION_PERIOD_SLOTS,
        }
    }
}

impl SnapPivotPolicy {
    /// Returns this policy anchoring `head_distance` blocks behind the head.
    pub const fn with_head_distance(mut self, head_distance: u64) -> Self {
        self.head_distance = head_distance;
        self
    }

    /// Returns this policy re-anchoring once a pivot lags by `advance_after` blocks.
    pub const fn with_advance_after(mut self, advance_after: u64) -> Self {
        self.advance_after = advance_after;
        self
    }

    /// Returns this policy assuming `history` blocks of block access lists remain servable.
    ///
    /// Defaults to the full EIP-7928 retention period, since applying lists beats downloading the
    /// state again.
    pub const fn with_history(mut self, history: u64) -> Self {
        self.history = history;
        self
    }

    /// Returns the block a pivot anchored under `head` targets.
    ///
    /// Prefers a finalized block that peers still serve state for, since it cannot be reorged, and
    /// falls back to the head distance.
    pub const fn pivot_block(&self, head: u64, finalized: Option<u64>) -> Option<u64> {
        if let Some(finalized) = finalized &&
            head.saturating_sub(finalized) <= self.advance_after
        {
            return Some(finalized)
        }
        head.checked_sub(self.head_distance)
    }

    /// Returns whether `generation` should be re-anchored under `head`.
    ///
    /// Advancing stays far cheaper than restarting, so this triggers well before peers stop
    /// serving the old root.
    pub const fn needs_advance(&self, generation: SnapGeneration, head: u64) -> bool {
        generation.lag(head) > self.advance_after
    }

    /// Returns whether the block access lists `generation` still needs remain servable.
    ///
    /// Once they are not, its state cannot be carried forward and the attempt has to restart.
    pub const fn is_catchable(&self, generation: SnapGeneration, head: u64) -> bool {
        generation.lag(head) <= self.history
    }

    /// Returns a fresh generation for the canonical pivot under `head`.
    ///
    /// A candidate that is not eligible falls back to the head distance; `None` means no candidate
    /// can anchor a sync yet.
    pub fn select(
        &self,
        provider: &impl HeaderProvider,
        head: u64,
        finalized: Option<u64>,
    ) -> Result<Option<SnapGeneration>, SnapSyncError> {
        let preferred = self.pivot_block(head, finalized);
        let fallback =
            head.checked_sub(self.head_distance).filter(|block| Some(*block) != preferred);
        for block_number in preferred.into_iter().chain(fallback) {
            let Some(header) = provider.sealed_header(block_number)? else { continue };
            if header.block_access_list_hash().is_some() {
                return Ok(Some(SnapGeneration::new(header.num_hash(), header.state_root())))
            }
        }
        Ok(None)
    }

    /// Returns whether an interrupted generation is still worth finishing under `head`.
    ///
    /// A fully downloaded generation only needs its trie rebuilt, so it always is.
    pub const fn is_finishable(&self, generation: SnapGeneration, head: u64) -> bool {
        matches!(generation.phase(), SnapPhase::Trie) || self.is_catchable(generation, head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{chain, policy, provider_with};
    use alloy_eips::BlockNumHash;
    use alloy_primitives::B256;

    #[test]
    fn selects_the_bal_capable_pivot_behind_the_head() {
        let headers = chain(Some(0));
        let expected = headers[2].clone();
        let provider = provider_with(headers);

        let generation = policy().select(&provider, 3, None).unwrap().unwrap();

        assert_eq!(generation.target().number, 2);
        assert_eq!(generation.target().hash, expected.hash_slow());
        assert_eq!(generation.state_root(), expected.state_root);
        assert_eq!(generation.phase(), SnapPhase::Accounts);
    }

    #[test]
    fn a_recent_finalized_block_is_anchored_to_instead_of_the_head_distance() {
        let headers = chain(Some(0));
        let expected = headers[1].clone();
        let provider = provider_with(headers);

        let generation = policy().select(&provider, 3, Some(1)).unwrap().unwrap();

        assert_eq!(generation.target().number, 1);
        assert_eq!(generation.target().hash, expected.hash_slow());
    }

    #[test]
    fn finality_stalled_outside_the_advance_window_falls_back_to_the_head_distance() {
        let headers = chain(Some(0));
        let fallback = headers[2].clone();
        let provider = provider_with(headers);
        // A finalized block two behind the head is outside this policy's advance window.
        let policy = policy().with_advance_after(1);

        let generation = policy.select(&provider, 3, Some(1)).unwrap().unwrap();

        // HEAD-1, not the stale finalized block 1.
        assert_eq!(generation.target().number, 2);
        assert_eq!(generation.target().hash, fallback.hash_slow());
    }

    #[test]
    fn an_ineligible_finalized_pivot_falls_back_to_the_head_distance() {
        // Block access lists only start at block 2, so the finalized block predates activation.
        let headers = chain(Some(2));
        let fallback = headers[2].clone();
        let provider = provider_with(headers);

        let generation = policy().select(&provider, 3, Some(1)).unwrap().unwrap();

        // HEAD-1, rather than waiting for finality to reach activation.
        assert_eq!(generation.target().number, 2);
        assert_eq!(generation.target().hash, fallback.hash_slow());
    }

    #[test]
    fn pivot_without_a_bal_commitment_is_not_selectable() {
        let provider = provider_with(chain(Some(3)));

        assert_eq!(policy().select(&provider, 3, None).unwrap(), None);
        // Neither the finalized anchor nor the fallback carries a commitment.
        assert_eq!(policy().select(&provider, 3, Some(1)).unwrap(), None);
    }

    #[test]
    fn pivot_beyond_downloaded_headers_is_not_selectable() {
        let provider = provider_with(chain(Some(0)));

        assert_eq!(policy().select(&provider, 9, None).unwrap(), None);
    }

    #[test]
    fn chain_shorter_than_the_head_distance_has_no_pivot() {
        let provider = provider_with(chain(Some(0)));

        assert_eq!(policy().with_head_distance(4).select(&provider, 0, None).unwrap(), None);
    }

    #[test]
    fn a_pivot_lagging_past_the_advance_window_is_re_anchored() {
        let policy = policy();
        let generation = SnapGeneration::new(BlockNumHash::new(0, B256::ZERO), B256::ZERO);

        assert!(!policy.needs_advance(generation, 4));
        assert!(policy.needs_advance(generation, 5));
    }

    #[test]
    fn generation_outside_the_bal_window_is_not_finishable() {
        let headers = chain(Some(0));
        let anchor = headers[1].clone();
        let provider = provider_with(headers);
        let generation =
            SnapGeneration::new(BlockNumHash::new(1, anchor.hash_slow()), anchor.state_root);
        let policy = policy();

        assert!(generation.is_canonical(&provider).unwrap());
        assert!(policy.is_finishable(generation, 9));
        assert!(!policy.is_finishable(generation, 10));
    }

    #[test]
    fn downloaded_state_finishes_outside_the_bal_window() {
        let anchor = chain(Some(0))[1].clone();
        let generation =
            SnapGeneration::new(BlockNumHash::new(1, anchor.hash_slow()), anchor.state_root)
                .with_phase(SnapPhase::Trie);

        assert!(policy().is_finishable(generation, 1_000));
    }

    #[test]
    fn reorged_anchor_is_not_canonical() {
        let provider = provider_with(chain(Some(0)));
        let generation =
            SnapGeneration::new(BlockNumHash::new(1, B256::repeat_byte(0xff)), B256::ZERO);

        assert!(!generation.is_canonical(&provider).unwrap());
    }
}
