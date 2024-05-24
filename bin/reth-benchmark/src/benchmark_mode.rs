//! The benchmark mode defines whether the benchmark should run for a specific range of blocks or as
//! a continuous stream of payloads.
use std::ops::RangeInclusive;

/// Whether or not the benchmark should run as a continuous stream of payloads.
#[derive(Debug, PartialEq, Eq)]
pub enum BenchmarkMode {
    // TODO: just include the start block in `Continuous`
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    Continuous,
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}
