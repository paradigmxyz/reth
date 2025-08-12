// bin/reth-bench/src/bench_mode.rs

//! The benchmark mode defines whether the benchmark should run for a closed or open range of
//! blocks.
use std::ops::RangeInclusive;

/// Whether or not the benchmark should run as a continuous stream of payloads.
#[derive(Debug, PartialEq, Eq)]
pub enum BenchMode {
    /// Run the benchmark as a continuous stream of payloads, starting from a specific block.
    Continuous {
        /// The block to start from.
        start: u64,
    },
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}

impl BenchMode {
    /// Check if the block number is in the range
    pub fn contains(&self, block_number: u64) -> bool {
        match self {
            Self::Continuous { start } => block_number >= *start,
            Self::Range(range) => range.contains(&block_number),
        }
    }

    /// Create a [`BenchMode`] from optional `from` and `to` fields.
    pub fn new(from: Option<u64>, to: Option<u64>) -> Result<Self, eyre::Error> {
        match (from, to) {
            (Some(from), Some(to)) => Ok(Self::Range(from..=to)),
            (Some(from), None) => Ok(Self::Continuous { start: from }),
            // If neither --from nor --to are provided, we run the benchmark continuously,
            // starting from block 0 as a default. This preserves the old behavior.
            (None, None) => Ok(Self::Continuous { start: 0 }),
            (None, Some(_)) => {
                // `to` without `from` is ambiguous.
                Err(eyre::eyre!("--to cannot be specified without --from."))
            }
        }
    }
}
