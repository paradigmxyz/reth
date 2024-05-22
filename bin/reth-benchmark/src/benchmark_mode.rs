use std::ops::RangeInclusive;

/// Whether or not the benchmark should run as a continuous stream of payloads.
#[derive(Debug, PartialEq, Eq)]
pub enum BenchmarkMode {
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    Continuous,
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}
