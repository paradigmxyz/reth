use crate::PruneMode;
use alloy_primitives::{BlockNumber, TxNumber};

/// Saves the pruning progress of a stage.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(any(test, feature = "test-utils"), derive(Default, arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct PruneCheckpoint {
    /// Highest pruned block number. If it's [None], the pruning for block `0` is not finished yet.
    pub block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub tx_number: Option<TxNumber>,
    /// Prune mode.
    pub prune_mode: PruneMode,
}
