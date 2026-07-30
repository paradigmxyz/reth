//! This exposes reth's storage settings over prometheus.

use metrics::gauge;

/// Contains storage settings information for the application.
#[derive(Debug, Clone)]
pub struct StorageSettingsInfo {
    /// Whether this node uses v2 storage layout.
    pub storage_v2: bool,
    /// The high-level pruning mode.
    pub pruning_mode: &'static str,
    /// The effective pruning configuration as JSON.
    pub prune_config: String,
}

impl StorageSettingsInfo {
    /// This exposes reth's storage settings over prometheus.
    pub fn register_storage_settings_metrics(&self) {
        let storage_v2 = if self.storage_v2 { "true" } else { "false" };
        let labels: [(&str, String); 3] = [
            ("storage_v2", storage_v2.to_string()),
            ("pruning_mode", self.pruning_mode.to_string()),
            ("prune_config", self.prune_config.clone()),
        ];

        let gauge = gauge!("storage_settings", &labels);
        gauge.set(1);
    }
}
