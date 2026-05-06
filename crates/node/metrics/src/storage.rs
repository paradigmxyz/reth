//! This exposes storage configuration information over prometheus.

use metrics::{describe_gauge, gauge};

/// The node's effective storage profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// Minimal storage profile.
    Minimal,
    /// Full node storage profile.
    Full,
    /// Archive storage profile.
    Archive,
    /// Custom storage profile.
    Custom,
}

impl StorageMode {
    const ALL: [Self; 4] = [Self::Minimal, Self::Full, Self::Archive, Self::Custom];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Full => "full",
            Self::Archive => "archive",
            Self::Custom => "custom",
        }
    }
}

/// Contains storage information for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageInfo {
    /// Whether the database is using v2 storage.
    pub storage_v2: bool,
    /// The node's effective storage profile.
    pub mode: StorageMode,
}

impl StorageInfo {
    /// Creates a new storage info value.
    pub const fn new(storage_v2: bool, mode: StorageMode) -> Self {
        Self { storage_v2, mode }
    }

    /// This exposes reth's storage configuration information over prometheus.
    pub fn register_storage_metrics(&self) {
        gauge!("storage.v2").set(bool_as_f64(self.storage_v2));
        gauge!("storage.v2_minimal")
            .set(bool_as_f64(self.storage_v2 && self.mode == StorageMode::Minimal));

        for mode in StorageMode::ALL {
            gauge!("storage.mode", "mode" => mode.as_str()).set(bool_as_f64(self.mode == mode));
        }
    }
}

/// Describes storage information metrics.
pub fn describe_storage_metrics() {
    describe_gauge!("storage.v2", "Whether the node is using v2 storage");
    describe_gauge!(
        "storage.v2_minimal",
        "Whether the node is using v2 storage with the minimal storage profile"
    );
    describe_gauge!(
        "storage.mode",
        "The effective storage profile, reported as one-hot gauges labeled by mode"
    );
}

const fn bool_as_f64(value: bool) -> f64 {
    if value {
        1.0
    } else {
        0.0
    }
}
