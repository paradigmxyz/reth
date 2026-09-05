//! Chooses the canonical block a Snap generation is anchored to.
//!
//! Peers serve block access lists for a bounded window of recent blocks, so a pivot is only useful
//! while its catch-up range still fits inside that window.

use crate::{error::db_error, SnapGeneration, SnapPhase, SnapSyncError};
use alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS;
use reth_primitives_traits::AlloyBlockHeader;
use reth_storage_api::HeaderProvider;

// EIP-8189 anchors the pivot at HEAD-64, clear of the reorg-prone tip.
const DEFAULT_HEAD_DISTANCE: u64 = 64;
// Re-anchoring at twice that distance moves the pivot forward by EIP-8189's typical K=64 blocks.
const DEFAULT_ADVANCE_AFTER: u64 = DEFAULT_HEAD_DISTANCE * 2;

/// Distance and history bounds that decide where a generation is anchored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapPivotPolicy {
    /// Blocks behind the canonical head at which a pivot is anchored.
    pub head_distance: u64,
    /// Pivot lag that triggers re-anchoring while account ranges are still downloading.
    pub advance_after: u64,
    /// Blocks of block access list history a peer is assumed to still serve.
    ///
    /// Applying that many lists is cheaper than downloading the state again, so the default is the
    /// full EIP-7928 retention period rather than a shorter catch-up bound.
    pub history: u64,
}

impl Default for SnapPivotPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapPivotPolicy {
    /// Creates the default policy in a const context.
    pub const fn new() -> Self {
        Self {
            head_distance: DEFAULT_HEAD_DISTANCE,
            advance_after: DEFAULT_ADVANCE_AFTER,
            history: BAL_RETENTION_PERIOD_SLOTS,
        }
    }

    /// Returns the block a pivot anchored under `head` targets.
    ///
    /// # Examples
    ///
    /// ```
    /// use reth_snap_sync::SnapPivotPolicy;
    ///
    /// let policy = SnapPivotPolicy::default();
    /// assert_eq!(policy.pivot_block(1_000), Some(936));
    /// assert_eq!(policy.pivot_block(4), None);
    /// ```
    pub const fn pivot_block(&self, head: u64) -> Option<u64> {
        head.checked_sub(self.head_distance)
    }

    /// Returns whether a generation anchored at `target` should be re-anchored under `head`.
    pub const fn needs_advance(&self, target: u64, head: u64) -> bool {
        head.saturating_sub(target) > self.advance_after
    }

    /// Returns whether the block access lists a generation still needs remain servable.
    pub const fn is_catchable(&self, target: u64, head: u64) -> bool {
        head.saturating_sub(target) <= self.history
    }

    /// Returns a fresh generation for the canonical pivot under `head`.
    ///
    /// `None` means the chain is not ready to be pivoted on: it is shorter than the head distance,
    /// its pivot header is not downloaded yet, or EIP-7928 is not active at the pivot, so no block
    /// access list can carry that state forward.
    pub fn select(
        &self,
        provider: &impl HeaderProvider,
        head: u64,
    ) -> Result<Option<SnapGeneration>, SnapSyncError> {
        let Some(block_number) = self.pivot_block(head) else { return Ok(None) };
        let Some(header) = provider.sealed_header(block_number).map_err(db_error)? else {
            return Ok(None)
        };
        if header.block_access_list_hash().is_none() {
            return Ok(None)
        }
        Ok(Some(SnapGeneration::new(block_number, header.hash(), header.state_root())))
    }

    /// Returns whether an interrupted generation is still worth finishing under `head`.
    ///
    /// A generation that has downloaded its full state only needs its trie rebuilt, so it stays
    /// worth finishing however far the head has moved on. Whether its anchor is still canonical is
    /// reported separately by [`is_canonical_anchor`](Self::is_canonical_anchor), because an
    /// orphaned anchor can be recovered instead of abandoned.
    pub const fn is_finishable(&self, generation: SnapGeneration, head: u64) -> bool {
        matches!(generation.phase, SnapPhase::Trie) ||
            self.is_catchable(generation.target_block, head)
    }

    /// Returns whether the block a generation is anchored to is still canonical.
    pub fn is_canonical_anchor(
        &self,
        provider: &impl HeaderProvider,
        generation: SnapGeneration,
    ) -> Result<bool, SnapSyncError> {
        let header = provider.sealed_header(generation.target_block).map_err(db_error)?;
        Ok(header.is_some_and(|header| header.hash() == generation.target_hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_provider::{
        test_utils::create_test_provider_factory, DatabaseProviderFactory,
        StaticFileProviderFactory, StaticFileWriter,
    };
    use reth_static_file_types::StaticFileSegment;

    // Small bounds keep header fixtures short without changing the policy's decisions.
    fn policy() -> SnapPivotPolicy {
        SnapPivotPolicy { head_distance: 1, advance_after: 4, history: 8 }
    }

    fn header(number: u64, parent_hash: B256, block_access_list_hash: Option<B256>) -> Header {
        Header {
            number,
            parent_hash,
            state_root: B256::repeat_byte(number as u8),
            block_access_list_hash,
            ..Default::default()
        }
    }

    fn provider_with(
        headers: impl IntoIterator<Item = Header>,
    ) -> impl HeaderProvider<Header = Header> {
        let factory = create_test_provider_factory();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        for header in headers {
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
        }
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);
        factory.database_provider_ro().unwrap()
    }

    fn chain(bal_from: Option<u64>) -> Vec<Header> {
        let mut headers = Vec::new();
        let mut parent = B256::ZERO;
        for number in 0..=3 {
            let bal =
                bal_from.filter(|from| number >= *from).map(|_| B256::with_last_byte(number as u8));
            let header = header(number, parent, bal);
            parent = header.hash_slow();
            headers.push(header);
        }
        headers
    }

    #[test]
    fn selects_the_bal_capable_pivot_behind_the_head() {
        let headers = chain(Some(0));
        let expected = headers[2].clone();
        let provider = provider_with(headers);

        let generation = policy().select(&provider, 3).unwrap().unwrap();

        assert_eq!(generation.target_block, 2);
        assert_eq!(generation.target_hash, expected.hash_slow());
        assert_eq!(generation.state_root, expected.state_root);
        assert_eq!(generation.phase, SnapPhase::Accounts);
        assert_eq!(generation.next_block, 3);
    }

    #[test]
    fn pivot_without_a_bal_commitment_is_not_selectable() {
        let provider = provider_with(chain(Some(3)));

        assert_eq!(policy().select(&provider, 3).unwrap(), None);
    }

    #[test]
    fn pivot_beyond_downloaded_headers_is_not_selectable() {
        let provider = provider_with(chain(Some(0)));

        assert_eq!(policy().select(&provider, 9).unwrap(), None);
    }

    #[test]
    fn chain_shorter_than_the_head_distance_has_no_pivot() {
        let provider = provider_with(chain(Some(0)));

        assert_eq!(
            SnapPivotPolicy { head_distance: 4, ..policy() }.select(&provider, 0).unwrap(),
            None
        );
    }

    #[test]
    fn generation_outside_the_bal_window_is_not_finishable() {
        let headers = chain(Some(0));
        let anchor = headers[1].clone();
        let provider = provider_with(headers);
        let generation = SnapGeneration::new(1, anchor.hash_slow(), anchor.state_root);
        let policy = policy();

        assert!(policy.is_canonical_anchor(&provider, generation).unwrap());
        assert!(policy.is_finishable(generation, 9));
        assert!(!policy.is_finishable(generation, 10));
    }

    #[test]
    fn downloaded_state_finishes_outside_the_bal_window() {
        let anchor = chain(Some(0))[1].clone();
        let mut generation = SnapGeneration::new(1, anchor.hash_slow(), anchor.state_root);
        generation.phase = SnapPhase::Trie;

        assert!(policy().is_finishable(generation, 1_000));
    }

    #[test]
    fn reorged_anchor_is_not_canonical() {
        let provider = provider_with(chain(Some(0)));
        let generation = SnapGeneration::new(1, B256::repeat_byte(0xff), B256::ZERO);

        assert!(!policy().is_canonical_anchor(&provider, generation).unwrap());
    }
}
