use serde::{Deserialize, Serialize};

use crate::{BlockNumber, U256};

/// Hardforks can be based on block numbers (pre-merge), TDD (Paris)
/// or timestamp (post-merge)
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForkKind {
    /// A fork's block number
    Block(BlockNumber),

    /// The terminal total difficulty (used by Paris fork)
    TTD(TerminalTotalDifficulty),

    /// The unix timestamp of a fork
    Time(u64),
}

/// This struct is used when it's needed to determine is a hardfork is active
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ForkDiscriminant {
    /// The block number
    pub block_number: BlockNumber,
    /// The total difficulty
    pub total_difficulty: U256,
    /// The timestamp
    pub timestamp: u64,
}

impl ForkDiscriminant {
    /// Returns a new [ForkDiscriminant]
    pub fn new(block_number: BlockNumber, total_difficulty: U256, timestamp: u64) -> Self {
        Self { block_number, total_difficulty, timestamp }
    }

    /// Return a [ForkDiscriminant] with the given timestamp
    pub fn timestamp(timestamp: u64) -> Self {
        Self { timestamp, ..Default::default() }
    }
}

impl From<ForkKind> for ForkDiscriminant {
    fn from(value: ForkKind) -> Self {
        match value {
            ForkKind::Block(block_number) => block_number.into(),
            ForkKind::TTD(tdd) => tdd.into(),
            ForkKind::Time(timestamp) => ForkDiscriminant::timestamp(timestamp),
        }
    }
}

impl From<BlockNumber> for ForkDiscriminant {
    fn from(value: BlockNumber) -> Self {
        Self { block_number: value, ..Default::default() }
    }
}

impl From<TerminalTotalDifficulty> for ForkDiscriminant {
    fn from(value: TerminalTotalDifficulty) -> Self {
        Self {
            block_number: value.block.unwrap_or_default(),
            total_difficulty: value.total_difficulty,
            ..Default::default()
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalTotalDifficulty {
    /// The terminal total difficulty
    pub total_difficulty: U256,
    /// The block number
    pub block: Option<BlockNumber>,
}

impl TerminalTotalDifficulty {
    pub fn new(total_difficulty: U256, block: Option<BlockNumber>) -> Self {
        Self { total_difficulty, block }
    }
}
