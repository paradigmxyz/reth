#![deny(missing_docs)]

use crate::{BlockNumber, B256, U256};
use serde::{Deserialize, Serialize};

/// Describes the current head block.
///
/// The head block is the highest fully synced block.
///
/// Note: This is a slimmed down version of [Header], primarily for communicating the highest block
/// with the P2P network and the RPC.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
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
