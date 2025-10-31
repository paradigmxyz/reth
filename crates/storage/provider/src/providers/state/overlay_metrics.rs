use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};

/// Metrics for overlay state provider operations.
#[derive(Clone, Metrics)]
#[metrics(scope = "storage.overlay_state_provider")]
pub(super) struct OverlayStateProviderMetrics {
    /// Time to create base database provider
    pub(super) provider_creation_duration: Histogram,
    /// Time to convert block hash to block number
    pub(super) block_hash_lookup_duration: Histogram,
    /// Time to fetch trie reverts from DB
    pub(super) trie_reverts_duration: Histogram,
    /// Time to validate that revert data is available
    pub(super) reverts_validation_duration: Histogram,
    /// Time to fetch state reverts from DB via `from_reverts`
    pub(super) from_reverts_duration: Histogram,
    /// Count of trie revert entries collected per invocation
    pub(super) trie_reverts_len: Histogram,
    /// Count of hashed state revert entries collected per invocation
    pub(super) state_reverts_len: Histogram,
    /// Total time in database_provider_ro()
    pub(super) total_database_provider_ro_duration: Histogram,
    /// Counter for when block_hash is not set (no reverts path)
    pub(super) block_hash_not_set: Counter,
    /// Counter for when reverts are not required (optimization skip)
    pub(super) reverts_not_required: Counter,
    /// Counter for when reverts are required (fetch path)
    pub(super) reverts_required: Counter,
    /// Counter for empty trie reverts (wasted query)
    pub(super) trie_reverts_empty: Counter,
    /// Counter for non-empty trie reverts (legitimate fetch)
    pub(super) trie_reverts_nonempty: Counter,
    /// Counter for empty state reverts (wasted query)
    pub(super) state_reverts_empty: Counter,
    /// Counter for non-empty state reverts (legitimate fetch)
    pub(super) state_reverts_nonempty: Counter,
    /// Histogram of checkpoint delta (how far behind checkpoint is)
    pub(super) checkpoint_delta: Histogram,
}

#[cfg(feature = "metrics")]
pub(super) struct OverlayMetricsTimer<'a> {
    histogram: &'a Histogram,
    start: std::time::Instant,
}

#[cfg(feature = "metrics")]
impl<'a> OverlayMetricsTimer<'a> {
    pub(super) fn start(histogram: &'a Histogram) -> Self {
        Self { histogram, start: std::time::Instant::now() }
    }
}

#[cfg(feature = "metrics")]
impl Drop for OverlayMetricsTimer<'_> {
    fn drop(&mut self) {
        self.histogram.record(self.start.elapsed().as_secs_f64());
    }
}
