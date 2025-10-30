use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for overlay state provider operations.
#[derive(Clone, Metrics)]
#[metrics(scope = "storage.overlay_state_provider")]
pub(super) struct OverlayStateProviderMetrics {
    /// Time to create base database provider
    pub(super) base_provider_creation_duration: Histogram,
    /// Time to convert block hash to block number
    pub(super) block_hash_lookup_duration: Histogram,
    /// Time to fetch trie reverts from DB
    pub(super) trie_reverts_duration: Histogram,
    /// Time to fetch state reverts from DB
    pub(super) state_reverts_duration: Histogram,
    /// Total time in database_provider_ro()
    pub(super) total_database_provider_ro_duration: Histogram,
}
