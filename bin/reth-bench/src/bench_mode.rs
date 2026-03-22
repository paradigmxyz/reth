//! The benchmark mode defines whether the benchmark should run for a closed or open range of
//! blocks.
use std::ops::RangeInclusive;

/// Whether or not the benchmark should run as a continuous stream of payloads.
#[derive(Debug, PartialEq, Eq)]
pub enum BenchMode {
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    Continuous(u64),
    /// Run the benchmark for a specific range of blocks.
    Range(RangeInclusive<u64>),
}

impl BenchMode {
    /// Check if the block number is in the range
    pub fn contains(&self, block_number: u64) -> bool {
        match self {
            Self::Continuous(start) => block_number >= *start,
            Self::Range(range) => range.contains(&block_number),
        }
    }

    /// Returns the total number of blocks in the benchmark, if known.
    ///
    /// For [`BenchMode::Range`] this is the length of the range.
    /// For [`BenchMode::Continuous`] the total is unbounded, so `None` is returned.
    pub const fn total_blocks(&self) -> Option<u64> {
        match self {
            Self::Continuous(_) => None,
            Self::Range(range) => {
                Some(range.end().saturating_sub(*range.start()).saturating_add(1))
            }
        }
    }

    /// Create a [`BenchMode`] from optional `from` and `to` fields.
    ///
    /// If only `--to` is provided, `from` is derived as `latest_block + 1`.
    pub const fn new(from: Option<u64>, to: Option<u64>, latest_block: u64) -> Self {
        // If neither `--from` nor `--to` are provided, we will run the benchmark continuously,
        // starting at the latest block.
        match (from, to) {
            (Some(from), Some(to)) => Self::Range(from..=to),
            (None, None) => Self::Continuous(latest_block),
            (Some(start), None) => Self::Continuous(start),
            (None, Some(to)) => Self::Range(latest_block + 1..=to),
        }
    }
}
