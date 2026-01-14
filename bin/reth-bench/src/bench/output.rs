//! Contains various benchmark output formats, either for logging or for
//! serialization to / from files.

use csv::Writer;
use eyre::OptionExt;
use reth_primitives_traits::constants::GIGAGAS;
use serde::{ser::SerializeStruct, Serialize};
use std::{path::Path, time::Duration};
use tracing::info;

/// This is the suffix for gas output csv files.
pub(crate) const GAS_OUTPUT_SUFFIX: &str = "total_gas.csv";

/// This is the suffix for combined output csv files.
pub(crate) const COMBINED_OUTPUT_SUFFIX: &str = "combined_latency.csv";

/// This is the suffix for new payload output csv files.
pub(crate) const NEW_PAYLOAD_OUTPUT_SUFFIX: &str = "new_payload_latency.csv";

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

/// This is another [`Serialize`] implementation for the [`NewPayloadResult`] struct, serializing
/// the duration as microseconds because the csv writer would fail otherwise.
impl Serialize for NewPayloadResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        // convert the time to microseconds
        let time = self.latency.as_micros();
        let mut state = serializer.serialize_struct("NewPayloadResult", 2)?;
        state.serialize_field("gas_used", &self.gas_used)?;
        state.serialize_field("latency", &time)?;
        state.end()
    }
}

/// This represents the combined results of a `newPayload` call and a `forkchoiceUpdated` call in
/// the benchmark, containing the gas used, the `newPayload` latency, and the `forkchoiceUpdated`
/// latency.
#[derive(Debug)]
pub(crate) struct CombinedResult {
    /// The block number of the block being processed.
    pub(crate) block_number: u64,
    /// The gas limit of the block.
    pub(crate) gas_limit: u64,
    /// The number of transactions in the block.
    pub(crate) transaction_count: u64,
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
            "Block {} (gas_limit: {}) at {:.4} Ggas/s, used {} gas. Combined: {:.4} Ggas/s. fcu: {:?}, newPayload: {:?}",
            self.block_number,
            self.gas_limit,
            self.new_payload_result.gas_per_second() / GIGAGAS as f64,
            self.new_payload_result.gas_used,
            self.combined_gas_per_second() / GIGAGAS as f64,
            self.fcu_latency,
            self.new_payload_result.latency
        )
    }
}

/// This is a [`Serialize`] implementation for the [`CombinedResult`] struct, serializing the
/// durations as microseconds because the csv writer would fail otherwise.
impl Serialize for CombinedResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        // convert the time to microseconds
        let fcu_latency = self.fcu_latency.as_micros();
        let new_payload_latency = self.new_payload_result.latency.as_micros();
        let total_latency = self.total_latency.as_micros();
        let mut state = serializer.serialize_struct("CombinedResult", 7)?;

        // flatten the new payload result because this is meant for CSV writing
        state.serialize_field("block_number", &self.block_number)?;
        state.serialize_field("gas_limit", &self.gas_limit)?;
        state.serialize_field("transaction_count", &self.transaction_count)?;
        state.serialize_field("gas_used", &self.new_payload_result.gas_used)?;
        state.serialize_field("new_payload_latency", &new_payload_latency)?;
        state.serialize_field("fcu_latency", &fcu_latency)?;
        state.serialize_field("total_latency", &total_latency)?;
        state.end()
    }
}

/// This represents a row of total gas data in the benchmark.
#[derive(Debug)]
pub(crate) struct TotalGasRow {
    /// The block number of the block being processed.
    pub(crate) block_number: u64,
    /// The number of transactions in the block.
    pub(crate) transaction_count: u64,
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
    pub(crate) fn new(rows: Vec<TotalGasRow>) -> eyre::Result<Self> {
        // the duration is obtained from the last row
        let total_duration = rows.last().map(|row| row.time).ok_or_eyre("empty results")?;
        let blocks_processed = rows.len() as u64;
        let total_gas_used: u64 = rows.into_iter().map(|row| row.gas_used).sum();
        let total_gas_per_second = total_gas_used as f64 / total_duration.as_secs_f64();

        Ok(Self { total_gas_used, total_duration, total_gas_per_second, blocks_processed })
    }

    /// Return the total gigagas per second.
    pub(crate) fn total_gigagas_per_second(&self) -> f64 {
        self.total_gas_per_second / GIGAGAS as f64
    }
}

/// Write benchmark results to CSV files.
///
/// Writes two files to the output directory:
/// - `combined_latency.csv`: Per-block latency results
/// - `total_gas.csv`: Per-block gas usage over time
pub(crate) fn write_benchmark_results(
    output_dir: &Path,
    gas_results: &[TotalGasRow],
    combined_results: Vec<CombinedResult>,
) -> eyre::Result<()> {
    let output_path = output_dir.join(COMBINED_OUTPUT_SUFFIX);
    info!("Writing engine api call latency output to file: {:?}", output_path);
    let mut writer = Writer::from_path(&output_path)?;
    for result in combined_results {
        writer.serialize(result)?;
    }
    writer.flush()?;

    let output_path = output_dir.join(GAS_OUTPUT_SUFFIX);
    info!("Writing total gas output to file: {:?}", output_path);
    let mut writer = Writer::from_path(&output_path)?;
    for row in gas_results {
        writer.serialize(row)?;
    }
    writer.flush()?;

    info!("Finished writing benchmark output files to {:?}.", output_dir);
    Ok(())
}

/// This serializes the `time` field of the [`TotalGasRow`] to microseconds.
///
/// This is essentially just for the csv writer, which would have headers
impl Serialize for TotalGasRow {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        // convert the time to microseconds
        let time = self.time.as_micros();
        let mut state = serializer.serialize_struct("TotalGasRow", 4)?;
        state.serialize_field("block_number", &self.block_number)?;
        state.serialize_field("transaction_count", &self.transaction_count)?;
        state.serialize_field("gas_used", &self.gas_used)?;
        state.serialize_field("time", &time)?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use csv::Writer;
    use std::io::BufRead;

    #[test]
    fn test_write_total_gas_row_csv() {
        let row = TotalGasRow {
            block_number: 1,
            transaction_count: 10,
            gas_used: 1_000,
            time: Duration::from_secs(1),
        };

        let mut writer = Writer::from_writer(vec![]);
        writer.serialize(row).unwrap();
        let result = writer.into_inner().unwrap();

        // parse into Lines
        let mut result = result.as_slice().lines();

        // assert header
        let expected_first_line = "block_number,transaction_count,gas_used,time";
        let first_line = result.next().unwrap().unwrap();
        assert_eq!(first_line, expected_first_line);

        let expected_second_line = "1,10,1000,1000000";
        let second_line = result.next().unwrap().unwrap();
        assert_eq!(second_line, expected_second_line);
    }
}
