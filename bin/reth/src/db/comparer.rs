use reth_config::config::PruneConfig;
use reth_db::database::Database;
use reth_interfaces::{RethError, RethResult};
use reth_primitives::PruneSegment;
use reth_provider::{ProviderFactory, PruneCheckpointReader};
use std::sync::Arc;
#[allow(dead_code)]
/// Struct for managing and validating pruning configuration.
struct PruningValidator<DB> {
    provider_factory: Arc<ProviderFactory<DB>>,
    prune_config: Arc<PruneConfig>,
}
#[allow(dead_code)]

impl<DB: Database> PruningValidator<DB> {
    /// Creates a new instance of the pruning validator.
    fn new(provider_factory: Arc<ProviderFactory<DB>>, prune_config: Arc<PruneConfig>) -> Self {
        Self { provider_factory, prune_config }
    }

    /// Validates the current pruning configuration against the database checkpoints.
    fn validate(&self) -> RethResult<()> {
        let db_provider = self
            .provider_factory
            .provider()
            .map_err(|e| RethError::Custom(format!("Failed to create database provider: {}", e)))?;

        let segments = &self.prune_config.segments;

        // Check Sender Recovery segment
        if let Some(configured_mode) = segments.sender_recovery {
            if let Some(prune_checkpoint) =
                db_provider.get_prune_checkpoint(PruneSegment::SenderRecovery)?
            {
                if prune_checkpoint.prune_mode != configured_mode {
                    return Err(RethError::Custom(format!(
                    "Pruning configuration mismatch for segment SenderRecovery. Expected: {:?}, Found in checkpoint: {:?}",
                    configured_mode, prune_checkpoint.prune_mode
                )));
                }
            }
        }

        if let Some(configured_mode) = segments.transaction_lookup {
            if let Some(prune_checkpoint) =
                db_provider.get_prune_checkpoint(PruneSegment::TransactionLookup)?
            {
                if prune_checkpoint.prune_mode != configured_mode {
                    return Err(RethError::Custom(format!(
                    "Pruning configuration mismatch for segment TransactionLookup. Expected: {:?}, Found in checkpoint: {:?}",
                    configured_mode, prune_checkpoint.prune_mode
                )));
                }
            }
        }

        // Check Receipts segment
        if let Some(configured_mode) = segments.receipts {
            if let Some(prune_checkpoint) =
                db_provider.get_prune_checkpoint(PruneSegment::Receipts)?
            {
                if prune_checkpoint.prune_mode != configured_mode {
                    return Err(RethError::Custom(format!(
                        "Pruning configuration mismatch for segment Receipts. Expected: {:?}, Found in checkpoint: {:?}",
                        configured_mode, prune_checkpoint.prune_mode
                    )));
                }
            }
        }

        // Check Account History segment
        if let Some(configured_mode) = segments.account_history {
            if let Some(prune_checkpoint) =
                db_provider.get_prune_checkpoint(PruneSegment::AccountHistory)?
            {
                if prune_checkpoint.prune_mode != configured_mode {
                    return Err(RethError::Custom(format!(
                        "Pruning configuration mismatch for segment AccountHistory. Expected: {:?}, Found in checkpoint: {:?}",
                        configured_mode, prune_checkpoint.prune_mode
                    )));
                }
            }
        }

        // Check Storage History segment
        if let Some(configured_mode) = segments.storage_history {
            if let Some(prune_checkpoint) =
                db_provider.get_prune_checkpoint(PruneSegment::StorageHistory)?
            {
                if prune_checkpoint.prune_mode != configured_mode {
                    return Err(RethError::Custom(format!(
                        "Pruning configuration mismatch for segment StorageHistory. Expected: {:?}, Found in checkpoint: {:?}",
                        configured_mode, prune_checkpoint.prune_mode
                    )));
                }
            }
        }

        // TODO: Receipt Log Filter
        // let receipts_log_filter_mode = segments.receipts_log_filter.0;
        // for (&address, &configured_mode) in
        // self.prune_config.segments.receipts_log_filter.0.iter() {
        //     if let Some(prune_checkpoint) =
        //         db_provider.get_prune_checkpoint_for_address(PruneSegment::Receipts, address)?
        //     {
        //         if prune_checkpoint.prune_mode != configured_mode {
        //             return Err(RethError::Custom(format!(
        //                 "Pruning configuration mismatch for segment Receipts (Log Filter) for
        // address {:?}. Expected: {:?}, Found in checkpoint: {:?}",
        // address, configured_mode, prune_checkpoint.prune_mode             )));
        //         }
        //     }
        // }

        Ok(())
    }
}
