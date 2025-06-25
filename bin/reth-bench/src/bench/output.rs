//! Contains various benchmark output formats, either for logging or for
//! serialization to / from files.

use reth_primitives_traits::constants::GIGAGAS;
use serde::{de::DeserializeOwned, ser::SerializeStruct, Deserialize, Serialize};
use std::{collections::HashMap, path::Path, time::Duration};

/// This is the suffix for gas output csv files.
pub(crate) const GAS_OUTPUT_SUFFIX: &str = "total_gas.csv";

/// This is the suffix for combined output csv files.
pub(crate) const COMBINED_OUTPUT_SUFFIX: &str = "combined_latency.csv";

/// This is the suffix for new payload output csv files.
pub(crate) const NEW_PAYLOAD_OUTPUT_SUFFIX: &str = "new_payload_latency.csv";

/// This is the suffix for baseline comparison output csv files.
pub(crate) const BASELINE_COMPARISON_OUTPUT_SUFFIX: &str = "baseline_comparison.csv";

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
        let mut state = serializer.serialize_struct("NewPayloadResult", 3)?;
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
            "Payload {} processed at {:.4} Ggas/s, used {} total gas. Combined gas per second: {:.4} Ggas/s. fcu latency: {:?}, newPayload latency: {:?}",
            self.block_number,
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
        let mut state = serializer.serialize_struct("CombinedResult", 5)?;

        // flatten the new payload result because this is meant for CSV writing
        state.serialize_field("block_number", &self.block_number)?;
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
        let mut state = serializer.serialize_struct("TotalGasRow", 3)?;
        state.serialize_field("block_number", &self.block_number)?;
        state.serialize_field("gas_used", &self.gas_used)?;
        state.serialize_field("time", &time)?;
        state.end()
    }
}

/// This represents baseline data loaded from a previous benchmark CSV file.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BaselineData {
    /// The block number.
    pub(crate) block_number: u64,
    /// The gas used in the baseline run.
    pub(crate) gas_used: u64,
    /// The newPayload latency from the baseline run in microseconds.
    pub(crate) new_payload_latency: u128,
    /// The forkchoice updated latency from the baseline run in microseconds (optional for
    /// new_payload_only benchmarks).
    pub(crate) fcu_latency: Option<u128>,
    /// The total latency from the baseline run in microseconds (optional for new_payload_only
    /// benchmarks).
    pub(crate) total_latency: Option<u128>,
}

/// This represents a comparison result between baseline and current benchmark.
#[derive(Debug)]
pub(crate) struct BaselineComparisonResult {
    /// The block number.
    pub(crate) block_number: u64,
    /// The baseline newPayload latency in microseconds.
    pub(crate) baseline_latency: u128,
    /// The current newPayload latency in microseconds.
    pub(crate) current_latency: u128,
    /// The absolute difference in latency (current - baseline) in microseconds.
    pub(crate) latency_diff: i128,
    /// The percentage difference ((current - baseline) / baseline * 100).
    pub(crate) percent_diff: f64,
}

impl std::fmt::Display for BaselineComparisonResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.percent_diff > 0.0 { "slower" } else { "faster" };
        write!(
            f,
            "Block {}: {:.2}% {} than baseline (baseline: {}μs, current: {}μs, diff: {}μs)",
            self.block_number,
            self.percent_diff.abs(),
            status,
            self.baseline_latency,
            self.current_latency,
            self.latency_diff
        )
    }
}

impl Serialize for BaselineComparisonResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        let mut state = serializer.serialize_struct("BaselineComparisonResult", 5)?;
        state.serialize_field("block_number", &self.block_number)?;
        state.serialize_field("baseline_latency", &self.baseline_latency)?;
        state.serialize_field("current_latency", &self.current_latency)?;
        state.serialize_field("latency_diff", &self.latency_diff)?;
        state.serialize_field("percent_diff", &self.percent_diff)?;
        state.end()
    }
}

/// Loads baseline data from a CSV file.
pub(crate) fn load_baseline_data(path: &Path) -> eyre::Result<HashMap<u64, BaselineData>> {
    let mut reader = csv::Reader::from_path(path)?;
    let mut baseline_map = HashMap::new();

    for result in reader.deserialize() {
        let record: BaselineData = result?;
        baseline_map.insert(record.block_number, record);
    }

    if baseline_map.is_empty() {
        return Err(eyre::eyre!("No baseline data found in file: {:?}", path));
    }

    Ok(baseline_map)
}

/// Creates a baseline comparison result from current and baseline data.
pub(crate) fn create_baseline_comparison(
    block_number: u64,
    current_latency: Duration,
    baseline_data: &BaselineData,
) -> BaselineComparisonResult {
    let current_latency_micros = current_latency.as_micros();
    let baseline_latency_micros = baseline_data.new_payload_latency;

    let latency_diff = current_latency_micros as i128 - baseline_latency_micros as i128;
    let percent_diff = if baseline_latency_micros > 0 {
        (latency_diff as f64 / baseline_latency_micros as f64) * 100.0
    } else {
        0.0
    };

    BaselineComparisonResult {
        block_number,
        baseline_latency: baseline_latency_micros,
        current_latency: current_latency_micros,
        latency_diff,
        percent_diff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use csv::Writer;
    use std::io::BufRead;

    #[test]
    fn test_write_total_gas_row_csv() {
        let row = TotalGasRow { block_number: 1, gas_used: 1_000, time: Duration::from_secs(1) };

        let mut writer = Writer::from_writer(vec![]);
        writer.serialize(row).unwrap();
        let result = writer.into_inner().unwrap();

        // parse into Lines
        let mut result = result.as_slice().lines();

        // assert header
        let expected_first_line = "block_number,gas_used,time";
        let first_line = result.next().unwrap().unwrap();
        assert_eq!(first_line, expected_first_line);

        let expected_second_line = "1,1000,1000000";
        let second_line = result.next().unwrap().unwrap();
        assert_eq!(second_line, expected_second_line);
    }
}
