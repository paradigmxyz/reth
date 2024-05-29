//! The benchmark mode defines whether the benchmark should run for a specific range of blocks or as
//! a continuous stream of payloads.
use std::ops::RangeInclusive;

/// Whether or not the benchmark should run as a continuous stream of payloads.
#[derive(Debug, PartialEq, Eq)]
pub enum BenchMode {
    // TODO: just include the start block in `Continuous`
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    Continuous,
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}

impl BenchMode {
    /// Check if the block number is in the range
    pub fn contains(&self, block_number: u64) -> bool {
        match self {
            Self::Continuous => true,
            Self::Range(range) => range.contains(&block_number),
        }
    }
}
