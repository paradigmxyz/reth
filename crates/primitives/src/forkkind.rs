use serde::{Deserialize, Serialize};

use crate::{BlockNumber, ChainSpec, U256};

/// Hardforks can be based on block numbers (pre-merge), TTD (Paris)
/// or timestamp (post-merge)
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForkKind {
    /// A fork's block number
    Block(BlockNumber),

    /// The terminal total difficulty (used by Paris fork)
    TTD(Option<BlockNumber>),

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

    /// Return a [ForkDiscriminant] with the given block
    pub fn block(block_number: BlockNumber) -> Self {
        Self { block_number, ..Default::default() }
    }

    /// Return a [ForkDiscriminant] with the given timestamp
    pub fn tdd(total_difficulty: U256, block_number: Option<BlockNumber>) -> Self {
        Self {
            block_number: block_number.unwrap_or_default(),
            total_difficulty,
            ..Default::default()
        }
    }

    /// Return a [ForkDiscriminant] with the given timestamp
    pub fn timestamp(timestamp: u64) -> Self {
        Self { timestamp, ..Default::default() }
    }

    /// Return a [ForkDiscriminant] from the given [ForkKind]
    pub fn from_kind(kind: ForkKind, chain_spec: &ChainSpec) -> Self {
        match kind {
            ForkKind::Block(block_number) => ForkDiscriminant::block(block_number),
            ForkKind::TTD(block_number) => {
                ForkDiscriminant::tdd(chain_spec.paris_ttd.unwrap_or_default(), block_number)
            }
            ForkKind::Time(timestamp) => ForkDiscriminant::timestamp(timestamp),
        }
    }
}

impl From<BlockNumber> for ForkDiscriminant {
    fn from(value: BlockNumber) -> Self {
        Self { block_number: value, ..Default::default() }
    }
}
