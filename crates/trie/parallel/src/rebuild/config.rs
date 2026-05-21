use super::StoragePrefixPlannerConfig;

/// Experimental parallel rebuild configuration.
#[derive(Debug, Clone, Copy)]
pub struct StorageRootPrefetchConfig {
    /// Maximum number of trie updates to keep before returning progress.
    pub progress_threshold: u64,
    /// Optional storage-prefix planner used for large storage tries.
    pub storage_prefix_planner: Option<StoragePrefixPlannerConfig>,
    /// Serial probe threshold before switching to storage-prefix planning.
    pub storage_prefix_trigger_threshold: u64,
    /// Minimum storage slot count required for storage-prefix planning.
    pub storage_prefix_min_large_slots: usize,
    /// Whether empty storage roots are handled inline on the account path.
    pub inline_empty_storage_roots: bool,
    /// Maximum slot count for inline small-storage root calculation.
    pub inline_storage_root_slot_limit: usize,
    /// Maximum account-prefix depth for the account segmented rebuild.
    pub account_prefix_max_depth: usize,
    /// Maximum number of account-prefix items to process per progress window.
    pub account_prefix_window_size: usize,
}

impl Default for StorageRootPrefetchConfig {
    fn default() -> Self {
        Self {
            progress_threshold: u64::MAX,
            storage_prefix_planner: None,
            storage_prefix_trigger_threshold: u64::MAX,
            storage_prefix_min_large_slots: usize::MAX,
            inline_empty_storage_roots: false,
            inline_storage_root_slot_limit: 0,
            account_prefix_max_depth: 2,
            account_prefix_window_size: 0,
        }
    }
}
