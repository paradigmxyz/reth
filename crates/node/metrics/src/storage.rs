//! This exposes reth's storage settings over prometheus.

use metrics::gauge;

/// Contains storage settings information for the application.
#[derive(Debug, Clone, Copy)]
pub struct StorageSettingsInfo {
    /// Whether this node uses v2 storage layout.
    pub storage_v2: bool,
    /// The high-level pruning mode.
    pub pruning_mode: &'static str,
}

impl StorageSettingsInfo {
    /// This exposes reth's storage settings over prometheus.
    pub fn register_storage_settings_metrics(&self) {
        let storage_v2 = if self.storage_v2 { "true" } else { "false" };
        let labels: [(&str, &str); 2] =
            [("storage_v2", storage_v2), ("pruning_mode", self.pruning_mode)];

        let gauge = gauge!("storage_settings", &labels);
        gauge.set(1);
    }
}
