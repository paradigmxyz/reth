use std::ops::RangeInclusive;

/// Whether or not the benchmark should run as a continuous stream of payloads.
///
/// TODO: build two structs that impl Stream, which return payloads from the local db and from the
/// rpc api.
///
/// impl rpc one first - the payload needs to be ready instantly
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BenchmarkMode {
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    Continuous,
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}
