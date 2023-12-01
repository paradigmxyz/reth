use alloy_primitives::{BlockNumber, B256, U256};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Describes the current head block.
///
/// The head block is the highest fully synced block.
///
/// Note: This is a slimmed down version of Header, primarily for communicating the highest block
/// with the P2P network and the RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Head {
    /// The number of the head block.
    pub number: BlockNumber,
    /// The hash of the head block.
    pub hash: B256,
    /// The difficulty of the head block.
    pub difficulty: U256,
    /// The total difficulty at the head block.
    pub total_difficulty: U256,
    /// The timestamp of the head block.
    pub timestamp: u64,
}
impl Head {
    /// Creates a new `Head` instance.
    pub fn new(
        number: BlockNumber,
        hash: B256,
        difficulty: U256,
        total_difficulty: U256,
        timestamp: u64,
    ) -> Self {
        Self { number, hash, difficulty, total_difficulty, timestamp }
    }

    /// Updates the head block with new information.
    pub fn update(
        &mut self,
        number: BlockNumber,
        hash: B256,
        difficulty: U256,
        total_difficulty: U256,
        timestamp: u64,
    ) {
        self.number = number;
        self.hash = hash;
        self.difficulty = difficulty;
        self.total_difficulty = total_difficulty;
        self.timestamp = timestamp;
    }

    /// Returns the hash of the head block.
    pub fn hash(&self) -> B256 {
        self.hash
    }

    /// Returns the block number of the head block.
    pub fn number(&self) -> BlockNumber {
        self.number
    }

    /// Returns the difficulty of the head block.
    pub fn difficulty(&self) -> U256 {
        self.difficulty
    }

    /// Returns the total difficulty of the blockchain up to the head block.
    pub fn total_difficulty(&self) -> U256 {
        self.total_difficulty
    }

    /// Returns the timestamp of the head block.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Checks if the head block is an empty block (i.e., has default values).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Head Block:\n Number: {}\n Hash: {:?}\n Difficulty: {:?}\n Total Difficulty: {:?}\n Timestamp: {}",
            self.number, self.hash, self.difficulty, self.total_difficulty, self.timestamp
        )
    }
}

impl Default for Head {
    fn default() -> Self {
        Self {
            number: 0,
            hash: B256::default(),
            difficulty: U256::default(),
            total_difficulty: U256::default(),
            timestamp: 0,
        }
    }
}
