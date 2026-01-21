//! # Storage Bloom Filter
//!
//! A bloom filter implementation for detecting empty storage slots before hitting `RocksDB`.
//!
//! ## Motivation
//!
//! Based on analysis from Nethermind's `FlatDB` work:
//! - 30-40% of storage slot reads return empty/zero values
//! - A bloom filter can short-circuit these reads, avoiding I/O entirely
//! - Potential 10-20% mgas/s improvement on some block ranges
//!
//! ## Design
//!
//! The bloom filter tracks which (address, slot) pairs have non-zero values.
//! - If bloom says "definitely not present" → return zero immediately (no DB read)
//! - If bloom says "maybe present" → proceed with normal DB lookup
//!
//! ## Trade-offs
//!
//! - **Memory**: ~2-3GB for mainnet scale with acceptable false positive rate
//! - **Write amplification**: Every storage write must update the bloom

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod bloom;
mod metrics;
mod provider;

pub use bloom::{StorageBloomConfig, StorageBloomFilter};
pub use provider::StorageBloomProvider;
