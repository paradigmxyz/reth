//! Pruning and full node arguments

use std::ops::Not;

use crate::primitives::EthereumHardfork;
use alloy_primitives::BlockNumber;
use clap::{builder::RangedU64ValueParser, Args};
use reth_chainspec::EthereumHardforks;
use reth_config::config::PruneConfig;
use reth_prune_types::{PruneMode, PruneModes, ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE};

/// Parameters for pruning and full node
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Pruning")]
pub struct PruningArgs {
    /// Run full node. Only the most recent [`MINIMUM_PRUNING_DISTANCE`] block states are stored.
    #[arg(long, default_value_t = false)]
    pub full: bool,

    /// Minimum pruning interval measured in blocks.
    #[arg(long = "prune.block-interval", alias = "block-interval", value_parser = RangedU64ValueParser::<u64>::new().range(1..))]
    pub block_interval: Option<u64>,

    // Sender Recovery
    /// Prunes all sender recovery data.
    #[arg(long = "prune.sender-recovery.full", alias = "prune.senderrecovery.full", conflicts_with_all = &["sender_recovery_distance", "sender_recovery_before"])]
    pub sender_recovery_full: bool,
    /// Prune sender recovery data before the `head-N` block number. In other words, keep last N +
    /// 1 blocks.
    #[arg(long = "prune.sender-recovery.distance", alias = "prune.senderrecovery.distance", value_name = "BLOCKS", conflicts_with_all = &["sender_recovery_full", "sender_recovery_before"])]
    pub sender_recovery_distance: Option<u64>,
    /// Prune sender recovery data before the specified block number. The specified block number is
    /// not pruned.
    #[arg(long = "prune.sender-recovery.before", alias = "prune.senderrecovery.before", value_name = "BLOCK_NUMBER", conflicts_with_all = &["sender_recovery_full", "sender_recovery_distance"])]
    pub sender_recovery_before: Option<BlockNumber>,

    // Transaction Lookup
    /// Prunes all transaction lookup data.
    #[arg(long = "prune.transaction-lookup.full", alias = "prune.transactionlookup.full", conflicts_with_all = &["transaction_lookup_distance", "transaction_lookup_before"])]
    pub transaction_lookup_full: bool,
    /// Prune transaction lookup data before the `head-N` block number. In other words, keep last N
    /// + 1 blocks.
    #[arg(long = "prune.transaction-lookup.distance", alias = "prune.transactionlookup.distance", value_name = "BLOCKS", conflicts_with_all = &["transaction_lookup_full", "transaction_lookup_before"])]
    pub transaction_lookup_distance: Option<u64>,
    /// Prune transaction lookup data before the specified block number. The specified block number
    /// is not pruned.
    #[arg(long = "prune.transaction-lookup.before", alias = "prune.transactionlookup.before", value_name = "BLOCK_NUMBER", conflicts_with_all = &["transaction_lookup_full", "transaction_lookup_distance"])]
    pub transaction_lookup_before: Option<BlockNumber>,

    // Receipts
    /// Prunes all receipt data.
    #[arg(long = "prune.receipts.full", conflicts_with_all = &["receipts_pre_merge", "receipts_distance", "receipts_before"])]
    pub receipts_full: bool,
    /// Prune receipts before the merge block.
    #[arg(long = "prune.receipts.pre-merge", conflicts_with_all = &["receipts_full", "receipts_distance", "receipts_before"])]
    pub receipts_pre_merge: bool,
    /// Prune receipts before the `head-N` block number. In other words, keep last N + 1 blocks.
    #[arg(long = "prune.receipts.distance", value_name = "BLOCKS", conflicts_with_all = &["receipts_full", "receipts_pre_merge", "receipts_before"])]
    pub receipts_distance: Option<u64>,
    /// Prune receipts before the specified block number. The specified block number is not pruned.
    #[arg(long = "prune.receipts.before", value_name = "BLOCK_NUMBER", conflicts_with_all = &["receipts_full", "receipts_pre_merge", "receipts_distance"])]
    pub receipts_before: Option<BlockNumber>,
    /// Receipts Log Filter
    #[arg(
        long = "prune.receipts-log-filter",
        alias = "prune.receiptslogfilter",
        value_name = "FILTER_CONFIG",
        hide = true
    )]
    #[deprecated]
    pub receipts_log_filter: Option<String>,

    // Account History
    /// Prunes all account history.
    #[arg(long = "prune.account-history.full", alias = "prune.accounthistory.full", conflicts_with_all = &["account_history_distance", "account_history_before"])]
    pub account_history_full: bool,
    /// Prune account before the `head-N` block number. In other words, keep last N + 1 blocks.
    #[arg(long = "prune.account-history.distance", alias = "prune.accounthistory.distance", value_name = "BLOCKS", conflicts_with_all = &["account_history_full", "account_history_before"])]
    pub account_history_distance: Option<u64>,
    /// Prune account history before the specified block number. The specified block number is not
    /// pruned.
    #[arg(long = "prune.account-history.before", alias = "prune.accounthistory.before", value_name = "BLOCK_NUMBER", conflicts_with_all = &["account_history_full", "account_history_distance"])]
    pub account_history_before: Option<BlockNumber>,

    // Storage History
    /// Prunes all storage history data.
    #[arg(long = "prune.storage-history.full", alias = "prune.storagehistory.full", conflicts_with_all = &["storage_history_distance", "storage_history_before"])]
    pub storage_history_full: bool,
    /// Prune storage history before the `head-N` block number. In other words, keep last N + 1
    /// blocks.
    #[arg(long = "prune.storage-history.distance", alias = "prune.storagehistory.distance", value_name = "BLOCKS", conflicts_with_all = &["storage_history_full", "storage_history_before"])]
    pub storage_history_distance: Option<u64>,
    /// Prune storage history before the specified block number. The specified block number is not
    /// pruned.
    #[arg(long = "prune.storage-history.before", alias = "prune.storagehistory.before", value_name = "BLOCK_NUMBER", conflicts_with_all = &["storage_history_full", "storage_history_distance"])]
    pub storage_history_before: Option<BlockNumber>,

    // Bodies
    /// Prune bodies before the merge block.
    #[arg(long = "prune.bodies.pre-merge", value_name = "BLOCKS", conflicts_with_all = &["bodies_distance", "bodies_before"])]
    pub bodies_pre_merge: bool,
    /// Prune bodies before the `head-N` block number. In other words, keep last N + 1
    /// blocks.
    #[arg(long = "prune.bodies.distance", value_name = "BLOCKS", conflicts_with_all = &["bodies_pre_merge", "bodies_before"])]
    pub bodies_distance: Option<u64>,
    /// Prune storage history before the specified block number. The specified block number is not
    /// pruned.
    #[arg(long = "prune.bodies.before", value_name = "BLOCK_NUMBER", conflicts_with_all = &["bodies_distance", "bodies_pre_merge"])]
    pub bodies_before: Option<BlockNumber>,
}

impl PruningArgs {
    /// Returns pruning configuration.
    ///
    /// Returns [`None`] if no parameters are specified and default pruning configuration should be
    /// used.
    pub fn prune_config<ChainSpec>(&self, chain_spec: &ChainSpec) -> Option<PruneConfig>
    where
        ChainSpec: EthereumHardforks,
    {
        // Initialize with a default prune configuration.
        let mut config = PruneConfig::default();

        // If --full is set, use full node defaults.
        if self.full {
            config = PruneConfig {
                block_interval: config.block_interval,
                segments: PruneModes {
                    sender_recovery: Some(PruneMode::Full),
                    transaction_lookup: None,
                    receipts: Some(PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)),
                    account_history: Some(PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)),
                    storage_history: Some(PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)),
                    bodies_history: chain_spec
                        .ethereum_fork_activation(EthereumHardfork::Paris)
                        .block_number()
                        .map(PruneMode::Before),
                    merkle_changesets: PruneMode::Distance(MINIMUM_PRUNING_DISTANCE),
                    receipts_log_filter: ReceiptsLogPruneConfig::empty(),
                },
            }
        }

        // Override with any explicitly set prune.* flags.
        if let Some(block_interval) = self.block_interval {
            config.block_interval = block_interval as usize;
        }
        if let Some(mode) = self.sender_recovery_prune_mode() {
            config.segments.sender_recovery = Some(mode);
        }
        if let Some(mode) = self.transaction_lookup_prune_mode() {
            config.segments.transaction_lookup = Some(mode);
        }
        if let Some(mode) = self.receipts_prune_mode(chain_spec) {
            config.segments.receipts = Some(mode);
        }
        if let Some(mode) = self.account_history_prune_mode() {
            config.segments.account_history = Some(mode);
        }
        if let Some(mode) = self.bodies_prune_mode(chain_spec) {
            config.segments.bodies_history = Some(mode);
        }
        if let Some(mode) = self.storage_history_prune_mode() {
            config.segments.storage_history = Some(mode);
        }

        // Log warning if receipts_log_filter is set (deprecated feature)
        #[expect(deprecated)]
        if self.receipts_log_filter.is_some() {
            tracing::warn!(
                target: "reth::cli",
                "The --prune.receiptslogfilter flag is deprecated and has no effect. It will be removed in a future release."
            );
        }

        config.is_default().not().then_some(config)
    }

    fn bodies_prune_mode<ChainSpec>(&self, chain_spec: &ChainSpec) -> Option<PruneMode>
    where
        ChainSpec: EthereumHardforks,
    {
        if self.bodies_pre_merge {
            chain_spec
                .ethereum_fork_activation(EthereumHardfork::Paris)
                .block_number()
                .map(PruneMode::Before)
        } else if let Some(distance) = self.bodies_distance {
            Some(PruneMode::Distance(distance))
        } else {
            self.bodies_before.map(PruneMode::Before)
        }
    }

    const fn sender_recovery_prune_mode(&self) -> Option<PruneMode> {
        if self.sender_recovery_full {
            Some(PruneMode::Full)
        } else if let Some(distance) = self.sender_recovery_distance {
            Some(PruneMode::Distance(distance))
        } else if let Some(block_number) = self.sender_recovery_before {
            Some(PruneMode::Before(block_number))
        } else {
            None
        }
    }

    const fn transaction_lookup_prune_mode(&self) -> Option<PruneMode> {
        if self.transaction_lookup_full {
            Some(PruneMode::Full)
        } else if let Some(distance) = self.transaction_lookup_distance {
            Some(PruneMode::Distance(distance))
        } else if let Some(block_number) = self.transaction_lookup_before {
            Some(PruneMode::Before(block_number))
        } else {
            None
        }
    }

    fn receipts_prune_mode<ChainSpec>(&self, chain_spec: &ChainSpec) -> Option<PruneMode>
    where
        ChainSpec: EthereumHardforks,
    {
        if self.receipts_pre_merge {
            chain_spec
                .ethereum_fork_activation(EthereumHardfork::Paris)
                .block_number()
                .map(PruneMode::Before)
        } else if self.receipts_full {
            Some(PruneMode::Full)
        } else if let Some(distance) = self.receipts_distance {
            Some(PruneMode::Distance(distance))
        } else {
            self.receipts_before.map(PruneMode::Before)
        }
    }

    const fn account_history_prune_mode(&self) -> Option<PruneMode> {
        if self.account_history_full {
            Some(PruneMode::Full)
        } else if let Some(distance) = self.account_history_distance {
            Some(PruneMode::Distance(distance))
        } else if let Some(block_number) = self.account_history_before {
            Some(PruneMode::Before(block_number))
        } else {
            None
        }
    }

    const fn storage_history_prune_mode(&self) -> Option<PruneMode> {
        if self.storage_history_full {
            Some(PruneMode::Full)
        } else if let Some(distance) = self.storage_history_distance {
            Some(PruneMode::Distance(distance))
        } else if let Some(block_number) = self.storage_history_before {
            Some(PruneMode::Before(block_number))
        } else {
            None
        }
    }
}
