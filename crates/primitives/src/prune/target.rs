use crate::{
    prune::PrunePartError, serde_helper::deserialize_opt_prune_mode_with_min_blocks, Address,
    BlockNumber, PruneMode, PrunePart,
};
use paste::paste;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Pruning configuration for every part of the data that can be pruned.
#[derive(Debug, Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneModes {
    /// Sender Recovery pruning configuration.
    // TODO(alexey): removing min blocks restriction is possible if we start calculating the senders
    //  dynamically on blockchain tree unwind.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub sender_recovery: Option<PruneMode>,
    /// Transaction Lookup pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_lookup: Option<PruneMode>,
    /// Configuration for pruning of receipts. This setting overrides
    /// `PruneModes::only_contract_logs` and offers improved performance.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub receipts: Option<PruneMode>,
    /// Account History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub storage_history: Option<PruneMode>,
    /// Retains only those receipts that contain logs emitted by the specified addresses,
    /// discarding all others. Note that this setting is overridden by `PruneModes::receipts`.
    ///
    /// The [`BlockNumber`] represents the starting block from which point onwards the receipts are
    /// preserved.
    pub only_contract_logs: ContractLogPart,
}

macro_rules! impl_prune_parts {
    ($(($part:ident, $variant:ident, $min_blocks:expr)),+) => {
        $(
            paste! {
                #[doc = concat!(
                    "Check if ",
                    stringify!($variant),
                    " should be pruned at the target block according to the provided tip."
                )]
                pub fn [<should_prune_ $part>](&self, block: BlockNumber, tip: BlockNumber) -> bool {
                    if let Some(mode) = &self.$part {
                        return mode.should_prune(block, tip)
                    }
                    false
                }
            }
        )+

        $(
            paste! {
                #[doc = concat!(
                    "Returns block up to which ",
                    stringify!($variant),
                    " pruning needs to be done, inclusive, according to the provided tip."
                )]
                pub fn [<prune_target_block_ $part>](&self, tip: BlockNumber) -> Result<Option<(BlockNumber, PruneMode)>, PrunePartError> {
                     match self.$part {
                        Some(mode) => mode.prune_target_block(tip, $min_blocks.unwrap_or_default(), PrunePart::$variant),
                        None => Ok(None)
                    }
                }
            }
        )+

        /// Sets pruning to all targets.
        pub fn all() -> Self {
            Self {
                $(
                    $part: Some(PruneMode::Full),
                )+
                only_contract_logs: Default::default()
            }
        }

    };
}

impl PruneModes {
    /// Sets pruning to no target.
    pub fn none() -> Self {
        PruneModes::default()
    }

    impl_prune_parts!(
        (sender_recovery, SenderRecovery, Some(64)),
        (transaction_lookup, TransactionLookup, None),
        (receipts, Receipts, Some(64)),
        (account_history, AccountHistory, Some(64)),
        (storage_history, StorageHistory, Some(64))
    );
}

/// Configuration for pruning receipts not associated with logs emitted by the specified contracts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContractLogPart(pub Vec<(PruneMode, Address)>);

impl ContractLogPart {
    /// Checks if the configuration is empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Given the `tip` block number, flatten the struct so it can easily be queried for filtering
    /// across a range of blocks.
    ///
    /// The [`BlockNumber`] key of the map should be viewed as `PruneMode::Before(block)`.
    pub fn flatten(
        &self,
        tip: BlockNumber,
    ) -> Result<BTreeMap<BlockNumber, Vec<&Address>>, PrunePartError> {
        let mut map = BTreeMap::new();
        for (mode, address) in self.0.iter() {
            let block = mode
                .prune_target_block(tip, 0, PrunePart::ContractLogs)?
                .map(|(block, _)| block)
                .unwrap_or_default();

            map.entry(block).or_insert_with(Vec::new).push(address)
        }
        Ok(map)
    }
}
