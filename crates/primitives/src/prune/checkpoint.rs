use crate::{prune::PruneMode, BlockNumber, TxNumber};
use reth_codecs::{main_codec, Compact};

/// Saves the pruning progress of a stage.
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(test, derive(Default))]
pub struct PruneCheckpoint {
    /// Highest pruned block number.
    /// If it's [None], the pruning for block `0` is not finished yet.
    pub block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub tx_number: Option<TxNumber>,
    /// Prune mode.
    pub prune_mode: PruneMode,
}
