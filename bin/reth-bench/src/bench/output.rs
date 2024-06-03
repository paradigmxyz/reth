//! Contains various benchmark output formats, either for logging or for
//! serialization to / from files.
//!
//! This also contains common constants for units, for example [GIGAGAS].

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Represents one Kilogas, or `1_000` gas.
const KILOGAS: u64 = 1_000;

/// Represents one Megagas, or `1_000_000` gas.
const MEGAGAS: u64 = KILOGAS * 1_000;

/// Represents one Gigagas, or `1_000_000_000` gas.
const GIGAGAS: u64 = MEGAGAS * 1_000;

/// This represents the results of a single `newPayload` call in the benchmark, containing the gas
/// used and the `newPayload` latency.
#[derive(Debug)]
pub(crate) struct NewPayloadResult {
    /// The gas used in the `newPayload` call.
    pub(crate) gas_used: u64,
    /// The latency of the `newPayload` call.
    pub(crate) latency: Duration,
}

impl NewPayloadResult {
    /// Returns the gas per second processed in the `newPayload` call.
    pub(crate) fn gas_per_second(&self) -> f64 {
        self.gas_used as f64 / self.latency.as_secs_f64()
    }
}

impl std::fmt::Display for NewPayloadResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "New payload processed at {:.4} Ggas/s, used {} total gas. Latency: {:?}",
            self.gas_per_second() / GIGAGAS as f64,
            self.gas_used,
            self.latency
        )
    }
}

/// This represents the combined results of a `newPayload` call and a `forkchoiceUpdated` call in
/// the benchmark, containing the gas used, the `newPayload` latency, and the `forkchoiceUpdated`
/// latency.
#[derive(Debug)]
pub(crate) struct CombinedResult {
    /// The `newPayload` result.
    pub(crate) new_payload_result: NewPayloadResult,
    /// The latency of the `forkchoiceUpdated` call.
    pub(crate) fcu_latency: Duration,
    /// The latency of both calls combined.
    pub(crate) total_latency: Duration,
}

impl CombinedResult {
    /// Returns the gas per second, including the `newPayload` _and_ `forkchoiceUpdated` duration.
    pub(crate) fn combined_gas_per_second(&self) -> f64 {
        self.new_payload_result.gas_used as f64 / self.total_latency.as_secs_f64()
    }
}

impl std::fmt::Display for CombinedResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Payload processed at {:.4} Ggas/s, used {} total gas. Combined gas per second: {:.4} Ggas/s. fcu latency: {:?}, newPayload latency: {:?}",
            self.new_payload_result.gas_per_second() / GIGAGAS as f64,
            self.new_payload_result.gas_used,
            self.combined_gas_per_second() / GIGAGAS as f64,
            self.fcu_latency,
            self.new_payload_result.latency
        )
    }
}

/// This represents a row of total gas data in the benchmark.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TotalGasRow {
    /// The block number of the block being processed.
    #[allow(dead_code)]
    pub(crate) block_number: u64,
    /// The total gas used in the block.
    pub(crate) gas_used: u64,
    /// Time since the start of the benchmark.
    pub(crate) time: Duration,
}

/// This represents the aggregated output, meant to show gas per second metrics, of a benchmark run.
#[derive(Debug)]
pub(crate) struct TotalGasOutput {
    /// The total gas used in the benchmark.
    pub(crate) total_gas_used: u64,
    /// The total duration of the benchmark.
    pub(crate) total_duration: Duration,
    /// The total gas used per second.
    pub(crate) total_gas_per_second: f64,
    /// The number of blocks processed.
    pub(crate) blocks_processed: u64,
}

impl TotalGasOutput {
    /// Create a new [`TotalGasOutput`] from a list of [`TotalGasRow`].
    pub(crate) fn new(rows: Vec<TotalGasRow>) -> Self {
        // the duration is obtained from the last row
        let total_duration =
            rows.last().map(|row| row.time).expect("the row has at least one element");
        let blocks_processed = rows.len() as u64;
        let total_gas_used: u64 = rows.into_iter().map(|row| row.gas_used).sum();
        let total_gas_per_second = total_gas_used as f64 / total_duration.as_secs_f64();

        Self { total_gas_used, total_duration, total_gas_per_second, blocks_processed }
    }

    /// Return the total gigagas per second.
    pub(crate) fn total_gigagas_per_second(&self) -> f64 {
        self.total_gas_per_second / GIGAGAS as f64
    }
}
