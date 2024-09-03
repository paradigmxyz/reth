use crate::PruneMode;
use alloy_primitives::{BlockNumber, TxNumber};
use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// Saves the pruning progress of a stage.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, Compact)]
#[cfg_attr(test, derive(Default, arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct PruneCheckpoint {
    /// Highest pruned block number. If it's [None], the pruning for block `0` is not finished yet.
    pub block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub tx_number: Option<TxNumber>,
    /// Prune mode.
    pub prune_mode: PruneMode,
}
