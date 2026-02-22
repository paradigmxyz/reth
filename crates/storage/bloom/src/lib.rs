//! In-memory bloom filter for short-circuiting empty storage slot reads.
//!
//! When the EVM executes `SLOAD`, 30-40% of reads target storage slots that have
//! never been written (Nethermind data). Each of these still requires a full MDBX
//! B-tree seek. This bloom filter returns a definitive "not present" answer in <400ns,
//! eliminating the seek entirely for those cases.
//!
//! # Design
//!
//! - **Lock-free**: Bit array uses [`AtomicU64`] with relaxed ordering — no contention between
//!   reader threads, no locks on the hot path.
//! - **Zero false negatives**: If `might_contain` returns `false`, the slot is guaranteed absent. A
//!   `true` result may be a false positive.
//! - **Insert-only**: Bits are never cleared. Deleted storage slots become false positives, causing
//!   a harmless fallback to MDBX. Rebuild on restart resets the filter.
//! - **Double hashing**: We extract two 64-bit values from `keccak256(address || slot)` and derive
//!   K probe positions via `h1 + i·h2 mod m`. This avoids multiple independent hash function calls.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloy_primitives::{keccak256, Address, B256};
use core::sync::atomic::{AtomicU64, Ordering};

/// Default number of hash functions.
const DEFAULT_NUM_HASHES: u8 = 3;

/// Magic bytes identifying a bloom filter file: "RETHBLOM"
#[cfg(feature = "std")]
const BLOOM_FILE_MAGIC: &[u8; 8] = b"RETHBLOM";

/// File format version. Bump this if the format changes.
#[cfg(feature = "std")]
const BLOOM_FILE_VERSION: u8 = 1;

/// A concurrent, insert-only bloom filter for `(Address, StorageSlot)` pairs.
///
/// Designed to sit in front of MDBX storage reads and short-circuit lookups
/// for slots that have never been written.
pub struct StorageBloomFilter {
    /// Bit array stored as atomic 64-bit words for lock-free access.
    bits: Box<[AtomicU64]>,
    /// Total number of usable bits (`bits.len() * 64`).
    num_bits: u64,
    /// Number of hash probe positions per key (K).
    num_hashes: u8,
}

impl StorageBloomFilter {
    /// Creates a new bloom filter with the given size in bytes and hash count.
    ///
    /// # Arguments
    /// - `size_bytes`: Total memory for the bit array. Must be >= 8 (one u64).
    /// - `num_hashes`: Number of probe positions per key (K). Typical: 3.
    ///
    /// # Panics
    /// Panics if `size_bytes < 8` or `num_hashes == 0`.
    pub fn new(size_bytes: usize, num_hashes: u8) -> Self {
        assert!(size_bytes >= 8, "bloom filter must be at least 8 bytes");
        assert!(num_hashes > 0, "bloom filter needs at least 1 hash function");
        let num_words = size_bytes / 8;
        let bits: Box<[AtomicU64]> = (0..num_words)
            .map(|_| AtomicU64::new(0))
            .collect::<alloc::vec::Vec<_>>()
            .into_boxed_slice();
        let num_bits = (num_words as u64) * 64;
        Self { bits, num_bits, num_hashes }
    }

    /// Creates a bloom filter with the given size in megabytes and default hash count.
    pub fn with_size_mb(size_mb: u64) -> Self {
        let size_bytes = (size_mb as usize) * 1024 * 1024;
        Self::new(size_bytes, DEFAULT_NUM_HASHES)
    }

    /// Hashes `address || slot` with keccak256 and returns `(h1, h2)` for double hashing.
    #[inline]
    fn hash_pair(address: &Address, slot: &B256) -> (u64, u64) {
        let mut buf = [0u8; 52];
        buf[..20].copy_from_slice(address.as_ref());
        buf[20..].copy_from_slice(slot.as_ref());
        let hash = keccak256(buf);
        let h1 = u64::from_le_bytes(hash[0..8].try_into().unwrap());
        let h2 = u64::from_le_bytes(hash[8..16].try_into().unwrap());
        (h1, h2)
    }

    /// Inserts an `(address, slot)` pair into the bloom filter.
    ///
    /// After this call, `might_contain(address, slot)` is guaranteed to return `true`.
    /// This operation is lock-free (atomic OR).
    #[inline]
    pub fn insert(&self, address: &Address, slot: &B256) {
        let (h1, h2) = Self::hash_pair(address, slot);
        for i in 0..self.num_hashes as u64 {
            let pos = h1.wrapping_add(i.wrapping_mul(h2)) % self.num_bits;
            let word_idx = (pos / 64) as usize;
            let bit_idx = pos % 64;
            self.bits[word_idx].fetch_or(1u64 << bit_idx, Ordering::Relaxed);
        }
    }

    /// Checks whether an `(address, slot)` pair *might* be in the filter.
    ///
    /// - Returns `false` → the slot is **guaranteed absent** (true negative).
    /// - Returns `true`  → the slot is **probably present** (may be a false positive).
    #[inline]
    pub fn might_contain(&self, address: &Address, slot: &B256) -> bool {
        let (h1, h2) = Self::hash_pair(address, slot);
        for i in 0..self.num_hashes as u64 {
            let pos = h1.wrapping_add(i.wrapping_mul(h2)) % self.num_bits;
            let word_idx = (pos / 64) as usize;
            let bit_idx = pos % 64;
            if self.bits[word_idx].load(Ordering::Relaxed) & (1u64 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }

    /// Returns the memory usage in bytes.
    pub fn memory_usage_bytes(&self) -> usize {
        self.bits.len() * 8
    }

    /// Returns the total number of bits set to 1.
    pub fn count_set_bits(&self) -> u64 {
        self.bits.iter().map(|w| w.load(Ordering::Relaxed).count_ones() as u64).sum()
    }

    /// Saves the bloom filter to a file.
    ///
    /// File format: `[magic: 8B][version: 1B][num_hashes: 1B][block_number: 8B][num_words: 8B][raw
    /// bits...]`
    ///
    /// The `block_number` is stored so that on restart we can incrementally catch up
    /// from changesets instead of re-scanning the entire `PlainStorageState` table.
    #[cfg(feature = "std")]
    pub fn save_to_file(&self, path: &std::path::Path, block_number: u64) -> std::io::Result<()> {
        use std::io::Write;

        let tmp = path.with_extension("tmp");
        let mut file = std::fs::File::create(&tmp)?;

        // Header
        file.write_all(BLOOM_FILE_MAGIC)?;
        file.write_all(&[BLOOM_FILE_VERSION])?;
        file.write_all(&[self.num_hashes])?;
        file.write_all(&block_number.to_le_bytes())?;
        let num_words = self.bits.len() as u64;
        file.write_all(&num_words.to_le_bytes())?;

        // Bit array — read each atomic word and write as LE bytes
        for word in &self.bits {
            file.write_all(&word.load(Ordering::Relaxed).to_le_bytes())?;
        }

        file.flush()?;
        drop(file);

        // Atomic rename to avoid partial reads on crash
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Loads a bloom filter from a file previously written by [`save_to_file`](Self::save_to_file).
    ///
    /// Returns `(bloom, block_number)` where `block_number` is the tip at which the filter
    /// was saved. The caller should catch up from `block_number + 1` to the current tip
    /// using storage changesets.
    ///
    /// Returns `Ok(None)` if the file does not exist or has an incompatible format/size.
    #[cfg(feature = "std")]
    pub fn load_from_file(
        path: &std::path::Path,
        expected_size_bytes: usize,
    ) -> std::io::Result<Option<(Self, u64)>> {
        use std::io::Read;

        if !path.exists() {
            return Ok(None);
        }

        let mut file = std::fs::File::open(path)?;

        // Read header
        let mut magic = [0u8; 8];
        if file.read_exact(&mut magic).is_err() {
            return Ok(None);
        }
        if magic != *BLOOM_FILE_MAGIC {
            return Ok(None);
        }

        let mut version = [0u8; 1];
        file.read_exact(&mut version)?;
        if version[0] != BLOOM_FILE_VERSION {
            return Ok(None);
        }

        let mut num_hashes_buf = [0u8; 1];
        file.read_exact(&mut num_hashes_buf)?;
        let num_hashes = num_hashes_buf[0];
        if num_hashes == 0 {
            return Ok(None);
        }

        let mut block_buf = [0u8; 8];
        file.read_exact(&mut block_buf)?;
        let block_number = u64::from_le_bytes(block_buf);

        let mut words_buf = [0u8; 8];
        file.read_exact(&mut words_buf)?;
        let num_words = u64::from_le_bytes(words_buf) as usize;

        // Validate size matches what we expect
        let expected_words = expected_size_bytes / 8;
        if num_words != expected_words {
            return Ok(None);
        }

        // Read bit array
        let mut bits: Box<[AtomicU64]> = (0..num_words)
            .map(|_| AtomicU64::new(0))
            .collect::<alloc::vec::Vec<_>>()
            .into_boxed_slice();

        let mut word_bytes = [0u8; 8];
        for word in &mut bits {
            file.read_exact(&mut word_bytes)?;
            *word = AtomicU64::new(u64::from_le_bytes(word_bytes));
        }

        let num_bits = (num_words as u64) * 64;
        let bloom = Self { bits, num_bits, num_hashes };
        Ok(Some((bloom, block_number)))
    }

    /// Returns the approximate false positive rate based on saturation.
    ///
    /// Formula: `(set_bits / total_bits) ^ num_hashes`
    #[cfg(feature = "std")]
    pub fn estimated_false_positive_rate(&self) -> f64 {
        let set_bits = self.count_set_bits();
        let saturation = set_bits as f64 / self.num_bits as f64;
        saturation.powi(self.num_hashes as i32)
    }

    /// Returns the total number of items that can be tracked before exceeding
    /// a given target false positive rate.
    ///
    /// Useful for deciding filter size.
    #[cfg(feature = "std")]
    pub fn capacity_at_fp_rate(&self, target_fp: f64) -> u64 {
        // n = -m * ln(1 - (fp^(1/k))) / k
        // where m = num_bits, k = num_hashes, fp = target false positive rate
        let k = self.num_hashes as f64;
        let m = self.num_bits as f64;
        let inner = 1.0 - target_fp.powf(1.0 / k);
        if inner <= 0.0 {
            return 0;
        }
        ((-m * inner.ln()) / k) as u64
    }
}

impl core::fmt::Debug for StorageBloomFilter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut s = f.debug_struct("StorageBloomFilter");
        s.field("size_mb", &(self.memory_usage_bytes() / (1024 * 1024)))
            .field("num_bits", &self.num_bits)
            .field("num_hashes", &self.num_hashes)
            .field("set_bits", &self.count_set_bits());
        #[cfg(feature = "std")]
        s.field("est_fp_rate", &self.estimated_false_positive_rate());
        s.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    #[test]
    fn insert_and_query() {
        let bloom = StorageBloomFilter::new(1024, 3); // 1KB
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        assert!(!bloom.might_contain(&addr, &slot));
        bloom.insert(&addr, &slot);
        assert!(bloom.might_contain(&addr, &slot));
    }

    #[test]
    fn absent_key_returns_false() {
        let bloom = StorageBloomFilter::new(1024 * 1024, 3); // 1MB - low FP rate
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let absent = b256!("0000000000000000000000000000000000000000000000000000000000000002");

        bloom.insert(&addr, &slot);
        // With 1MB filter and 1 inserted key, the absent key should almost certainly be false
        assert!(!bloom.might_contain(&addr, &absent));
    }

    #[test]
    fn empty_bloom_returns_false() {
        let bloom = StorageBloomFilter::new(64, 3); // minimum size
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        assert!(!bloom.might_contain(&addr, &slot));
        assert_eq!(bloom.count_set_bits(), 0);
    }

    #[test]
    fn false_positive_rate_stays_low() {
        // Insert 10K keys into a 1MB filter, expect FP rate < 1%
        let bloom = StorageBloomFilter::new(1024 * 1024, 3);
        let base_addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        // Insert 10K distinct slots
        for i in 0u64..10_000 {
            let slot = B256::from(alloy_primitives::U256::from(i));
            bloom.insert(&base_addr, &slot);
        }

        // Check 10K absent slots (different address)
        let other_addr = address!("1111111111111111111111111111111111111111");
        let mut false_positives = 0u64;
        for i in 0u64..10_000 {
            let slot = B256::from(alloy_primitives::U256::from(i));
            if bloom.might_contain(&other_addr, &slot) {
                false_positives += 1;
            }
        }

        let fp_rate = false_positives as f64 / 10_000.0;
        assert!(
            fp_rate < 0.01,
            "false positive rate too high: {fp_rate:.4} ({false_positives}/10000)"
        );
    }

    #[test]
    fn no_false_negatives() {
        // Every inserted key must be found
        let bloom = StorageBloomFilter::new(1024 * 1024, 3);
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        let slots: Vec<B256> =
            (0u64..1000).map(|i| B256::from(alloy_primitives::U256::from(i))).collect();

        for slot in &slots {
            bloom.insert(&addr, slot);
        }

        for slot in &slots {
            assert!(bloom.might_contain(&addr, slot), "false negative for slot {slot}");
        }
    }

    #[test]
    fn thread_safety() {
        use std::{sync::Arc, thread};

        let bloom = Arc::new(StorageBloomFilter::new(1024 * 1024, 3));
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        // Spawn 4 writer threads
        let mut handles = vec![];
        for t in 0..4u64 {
            let bloom = Arc::clone(&bloom);
            handles.push(thread::spawn(move || {
                for i in 0..1000u64 {
                    let slot = B256::from(alloy_primitives::U256::from(t * 1000 + i));
                    bloom.insert(&addr, &slot);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // All 4000 keys must be present
        for t in 0..4u64 {
            for i in 0..1000u64 {
                let slot = B256::from(alloy_primitives::U256::from(t * 1000 + i));
                assert!(bloom.might_contain(&addr, &slot));
            }
        }
    }

    #[test]
    fn memory_usage_matches() {
        let bloom = StorageBloomFilter::with_size_mb(1);
        assert_eq!(bloom.memory_usage_bytes(), 1024 * 1024);
    }

    #[test]
    fn debug_format() {
        let bloom = StorageBloomFilter::new(1024, 3);
        let debug = format!("{bloom:?}");
        assert!(debug.contains("StorageBloomFilter"));
        assert!(debug.contains("num_hashes: 3"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("reth_bloom_test_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bloom.bin");

        let bloom = StorageBloomFilter::new(1024, 3); // 1KB
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        bloom.insert(&addr, &slot);

        bloom.save_to_file(&path, 42).unwrap();

        let (loaded, block) = StorageBloomFilter::load_from_file(&path, 1024).unwrap().unwrap();
        assert_eq!(block, 42);
        assert!(loaded.might_contain(&addr, &slot));
        assert_eq!(loaded.memory_usage_bytes(), bloom.memory_usage_bytes());
        assert_eq!(loaded.count_set_bits(), bloom.count_set_bits());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_returns_none() {
        let path = std::path::Path::new("/tmp/reth_bloom_nonexistent.bin");
        let result = StorageBloomFilter::load_from_file(path, 1024).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_wrong_size_returns_none() {
        let dir = std::env::temp_dir().join("reth_bloom_test_wrong_size");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bloom.bin");

        let bloom = StorageBloomFilter::new(1024, 3); // 1KB = 128 words
        bloom.save_to_file(&path, 0).unwrap();

        // Try to load with a different expected size
        let result = StorageBloomFilter::load_from_file(&path, 2048).unwrap();
        assert!(result.is_none(), "should reject mismatched size");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn false_positive_rate_vs_num_hashes() {
        // Sweep K=1..8 to show how FP rate changes with num_hashes.
        // Run with: cargo test -p reth-storage-bloom false_positive_rate_vs_num_hashes --
        // --nocapture
        let size_bytes = 1024 * 1024; // 1MB filter
        let n_insert = 100_000u64;
        let n_query = 100_000u64;

        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let other = address!("1111111111111111111111111111111111111111");

        println!("\n{:<12} {:<15} {:<10}", "num_hashes", "false_positives", "fp_rate");
        println!("{}", "-".repeat(40));

        for k in 1u8..=8 {
            let bloom = StorageBloomFilter::new(size_bytes, k);

            for i in 0..n_insert {
                let slot = B256::from(alloy_primitives::U256::from(i));
                bloom.insert(&addr, &slot);
            }

            let mut fp = 0u64;
            for i in 0..n_query {
                let slot = B256::from(alloy_primitives::U256::from(i));
                if bloom.might_contain(&other, &slot) {
                    fp += 1;
                }
            }

            let rate = fp as f64 / n_query as f64;
            println!("{:<12} {:<15} {:.6}", k, fp, rate);
        }
    }
}
