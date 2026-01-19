//! Storage bloom filter implementation.

use alloy_primitives::{Address, StorageKey};
use growable_bloom_filter::GrowableBloom;
use parking_lot::RwLock;
use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
};
use tracing::info;

use crate::metrics::StorageBloomMetrics;

/// Configuration for the storage bloom filter.
#[derive(Debug, Clone)]
pub struct StorageBloomConfig {
    /// Target false positive rate (e.g., 0.01 = 1%).
    pub false_positive_rate: f64,
    /// Expected number of items (storage slots with non-zero values).
    /// Mainnet has ~500M-1B non-empty slots.
    pub expected_items: usize,
    /// Whether to enable the bloom filter.
    pub enabled: bool,
}

impl Default for StorageBloomConfig {
    fn default() -> Self {
        Self {
            // 1% false positive rate - reasonable trade-off
            false_positive_rate: 0.01,
            // ~800M slots expected for mainnet
            expected_items: 800_000_000,
            enabled: true,
        }
    }
}

impl StorageBloomConfig {
    /// Create a disabled config (for testing or when bloom is not wanted).
    pub const fn disabled() -> Self {
        Self { false_positive_rate: 0.01, expected_items: 1000, enabled: false }
    }

    /// Estimate memory usage in bytes.
    pub fn estimated_memory_bytes(&self) -> usize {
        // Bloom filter bits = -n * ln(p) / (ln(2)^2)
        // For 800M items at 1% FPR: ~7.6 billion bits = ~950MB
        // GrowableBloom adds some overhead, estimate ~1.2x
        let bits =
            (-1.0 * self.expected_items as f64 * self.false_positive_rate.ln() / (2.0_f64.ln().powi(2))) as usize;
        (bits / 8) * 12 / 10 // bits to bytes with 20% overhead
    }
}

/// A bloom filter for detecting empty storage slots.
///
/// This filter tracks which (address, storage_key) pairs have non-zero values.
/// It can definitively say "not present" but may have false positives for "maybe present".
pub struct StorageBloomFilter {
    /// The underlying bloom filter.
    bloom: RwLock<GrowableBloom>,
    /// Configuration.
    config: StorageBloomConfig,
    /// Metrics.
    metrics: StorageBloomMetrics,
}

impl StorageBloomFilter {
    /// Create a new storage bloom filter with the given configuration.
    pub fn new(config: StorageBloomConfig) -> Self {
        let bloom = GrowableBloom::new(config.false_positive_rate, config.expected_items);

        info!(
            target: "storage::bloom",
            expected_items = config.expected_items,
            fpr = config.false_positive_rate,
            estimated_memory_mb = config.estimated_memory_bytes() / 1_000_000,
            "Created storage bloom filter"
        );

        Self { bloom: RwLock::new(bloom), config, metrics: StorageBloomMetrics::default() }
    }

    /// Create a bloom filter from a persisted file.
    pub fn load_from_file(path: &Path, config: StorageBloomConfig) -> Result<Self, BloomError> {
        let file = File::open(path).map_err(BloomError::Io)?;
        let reader = BufReader::new(file);
        let bloom: GrowableBloom = bincode::deserialize_from(reader).map_err(BloomError::Deserialize)?;

        info!(
            target: "storage::bloom",
            path = %path.display(),
            "Loaded storage bloom filter from file"
        );

        Ok(Self { bloom: RwLock::new(bloom), config, metrics: StorageBloomMetrics::default() })
    }

    /// Persist the bloom filter to a file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), BloomError> {
        let bloom = self.bloom.read();
        let file = File::create(path).map_err(BloomError::Io)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, &*bloom).map_err(BloomError::Serialize)?;

        info!(
            target: "storage::bloom",
            path = %path.display(),
            "Saved storage bloom filter to file"
        );

        Ok(())
    }

    /// Check if a storage slot might have a non-zero value.
    ///
    /// Returns:
    /// - `false`: Definitely not present (slot is empty, no DB read needed)
    /// - `true`: Maybe present (need to check DB)
    #[inline]
    pub fn maybe_contains(&self, address: Address, storage_key: StorageKey) -> bool {
        if !self.config.enabled {
            return true; // Always check DB if disabled
        }

        let key = Self::make_key(address, storage_key);
        let result = self.bloom.read().contains(&key);

        if result {
            self.metrics.bloom_misses.increment(1);
        } else {
            self.metrics.bloom_hits.increment(1);
        }

        result
    }

    /// Insert a storage slot into the bloom filter.
    ///
    /// Call this when a non-zero value is written to storage.
    #[inline]
    pub fn insert(&self, address: Address, storage_key: StorageKey) {
        if !self.config.enabled {
            return;
        }

        let key = Self::make_key(address, storage_key);
        self.bloom.write().insert(&key);
        self.metrics.bloom_inserts.increment(1);
    }

    /// Record a false positive (bloom said maybe present, but DB returned empty).
    #[inline]
    pub fn record_false_positive(&self) {
        self.metrics.bloom_false_positives.increment(1);
    }

    /// Check if the filter is enabled.
    #[inline]
    pub const fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the configuration.
    pub const fn config(&self) -> &StorageBloomConfig {
        &self.config
    }

    /// Create a composite key from address and storage key.
    #[inline]
    fn make_key(address: Address, storage_key: StorageKey) -> [u8; 52] {
        let mut key = [0u8; 52];
        key[..20].copy_from_slice(address.as_slice());
        key[20..52].copy_from_slice(storage_key.as_slice());
        key
    }
}

impl std::fmt::Debug for StorageBloomFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageBloomFilter")
            .field("config", &self.config)
            .field("enabled", &self.config.enabled)
            .finish_non_exhaustive()
    }
}

/// Errors that can occur with bloom filter operations.
#[derive(Debug)]
pub enum BloomError {
    /// I/O error reading/writing bloom filter file.
    Io(std::io::Error),
    /// Error deserializing bloom filter.
    Deserialize(bincode::Error),
    /// Error serializing bloom filter.
    Serialize(bincode::Error),
}

impl std::fmt::Display for BloomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "bloom filter I/O error: {e}"),
            Self::Deserialize(e) => write!(f, "bloom filter deserialize error: {e}"),
            Self::Serialize(e) => write!(f, "bloom filter serialize error: {e}"),
        }
    }
}

impl std::error::Error for BloomError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Deserialize(e) => Some(e),
            Self::Serialize(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_bloom_basic_operations() {
        let config = StorageBloomConfig { expected_items: 1000, false_positive_rate: 0.01, enabled: true };

        let bloom = StorageBloomFilter::new(config);

        let addr = Address::repeat_byte(0x42);
        let slot1 = B256::repeat_byte(0x01);
        let slot2 = B256::repeat_byte(0x02);

        // Before insert, should return false (not present)
        assert!(!bloom.maybe_contains(addr, slot1));

        // Insert slot1
        bloom.insert(addr, slot1);

        // After insert, should return true (maybe present)
        assert!(bloom.maybe_contains(addr, slot1));

        // slot2 should still return false
        assert!(!bloom.maybe_contains(addr, slot2));
    }

    #[test]
    fn test_bloom_disabled() {
        let config = StorageBloomConfig::disabled();
        let bloom = StorageBloomFilter::new(config);

        let addr = Address::repeat_byte(0x42);
        let slot = B256::repeat_byte(0x01);

        // When disabled, always returns true (check DB)
        assert!(bloom.maybe_contains(addr, slot));
    }

    #[test]
    fn test_bloom_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let bloom_path = temp_dir.path().join("test_bloom.bin");

        let config =
            StorageBloomConfig { expected_items: 1000, false_positive_rate: 0.01, enabled: true };

        let addr = Address::repeat_byte(0x42);
        let slot = B256::repeat_byte(0x01);

        // Create and populate bloom
        {
            let bloom = StorageBloomFilter::new(config.clone());
            bloom.insert(addr, slot);
            bloom.save_to_file(&bloom_path).unwrap();
        }

        // Load and verify
        {
            let bloom = StorageBloomFilter::load_from_file(&bloom_path, config).unwrap();
            assert!(bloom.maybe_contains(addr, slot));
        }
    }

    #[test]
    fn test_estimated_memory() {
        let config = StorageBloomConfig::default();
        let memory_mb = config.estimated_memory_bytes() / 1_000_000;

        // Should be in the ~1-2GB range for 800M items at 1% FPR
        assert!(memory_mb > 500, "Expected >500MB, got {memory_mb}MB");
        assert!(memory_mb < 3000, "Expected <3000MB, got {memory_mb}MB");
    }
}
