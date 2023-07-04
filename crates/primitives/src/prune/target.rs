use crate::{BlockNumber, PruneMode};
use paste::paste;

/// Prune target.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PruneTarget {
    /// Prune all blocks, i.e. not save any data.
    All,
    /// Prune blocks up to the specified block number, inclusive.
    Block(BlockNumber),
}

impl PruneTarget {
    /// Returns new target to prune towards, according to stage prune mode [PruneMode]
    /// and current head [BlockNumber].
    pub fn new(prune_mode: PruneMode, head: BlockNumber) -> Self {
        match prune_mode {
            PruneMode::Full => PruneTarget::All,
            PruneMode::Distance(distance) => {
                Self::Block(head.saturating_sub(distance).saturating_sub(1))
            }
            PruneMode::Before(before_block) => Self::Block(before_block.saturating_sub(1)),
        }
    }

    /// Returns true if the target is [PruneTarget::All], i.e. prune all blocks.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }
}

/// Pruning configuration for every part of the data that can be pruned.
#[derive(Debug, Clone, Default, Copy, Eq, PartialEq)]
pub struct PruneTargets {
    /// Sender Recovery pruning configuration.
    pub sender_recovery: Option<PruneTarget>,
    /// Transaction Lookup pruning configuration.
    pub transaction_lookup: Option<PruneTarget>,
    /// Receipts pruning configuration.
    pub receipts: Option<PruneTarget>,
    /// Account History pruning configuration.
    pub account_history: Option<PruneTarget>,
    /// Storage History pruning configuration.
    pub storage_history: Option<PruneTarget>,
}

macro_rules! should_prune_method {
    ($($config:ident),+) => {
        $(
            paste! {
                #[allow(missing_docs)]
                pub fn [<should_prune_ $config>](&self, block: BlockNumber) -> bool {
                    if let Some(config) = &self.$config {
                        return self.should_prune(config, block)
                    }
                    false
                }
            }
        )+
    };
}

impl PruneTargets {
    /// Sets pruning to all targets.
    pub fn all() -> PruneTargets {
        PruneTargets {
            sender_recovery: Some(PruneTarget::All),
            transaction_lookup: Some(PruneTarget::All),
            receipts: Some(PruneTarget::All),
            account_history: Some(PruneTarget::All),
            storage_history: Some(PruneTarget::All),
        }
    }

    /// Sets pruning to no target.
    pub fn none() -> PruneTargets {
        PruneTargets::default()
    }

    /// Check if target block should be pruned
    pub fn should_prune(&self, target: &PruneTarget, block: BlockNumber) -> bool {
        match target {
            PruneTarget::All => true,
            PruneTarget::Block(n) => *n >= block,
        }
    }

    should_prune_method!(
        sender_recovery,
        transaction_lookup,
        receipts,
        account_history,
        storage_history
    );
}
