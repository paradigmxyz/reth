use crate::{
    db_ext::DbTxPruneExt,
    segments::{user::history::prune_history_indices, PruneInput, Segment},
    PrunerError,
};
use itertools::Itertools;
use reth_db_api::{models::ShardedKey, tables, transaction::DbTxMut};
use reth_provider::{DBProvider, EitherWriter, StaticFileProviderFactory, StorageSettingsCache};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::ChangeSetReader;
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct AccountHistory {
    mode: PruneMode,
}

impl AccountHistory {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for AccountHistory
where
    Provider: DBProvider<Tx: DbTxMut>
        + StaticFileProviderFactory
        + StorageSettingsCache
        + ChangeSetReader,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::AccountHistory
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(target = "pruner", skip(self, provider), ret(level = "trace"))]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No account history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        // Check where account changesets are stored
        if EitherWriter::account_changesets_destination(provider).is_static_file() {
            self.prune_static_files(provider, range, range_end, input.limiter)
        } else {
            self.prune_database(provider, range, range_end, input.limiter)
        }
    }
}

impl AccountHistory {
    fn prune_static_files<Provider>(
        &self,
        provider: &Provider,
        range: std::ops::RangeInclusive<alloy_primitives::BlockNumber>,
        range_end: alloy_primitives::BlockNumber,
        limiter: crate::segments::PruneLimiter,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider<Tx: DbTxMut> + StaticFileProviderFactory + ChangeSetReader,
    {
        // Read all account changesets from static files in the range
        let changesets = provider.account_changesets_range(range)?;

        // Build map of highest deleted block per account address
        let mut highest_deleted_accounts: FxHashMap<_, _> = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        for (block_number, changeset) in &changesets {
            highest_deleted_accounts.insert(changeset.address, *block_number);
            last_changeset_pruned_block = Some(*block_number);
        }

        let pruned_changesets = changesets.len();

        // Delete static file jars below the pruned block
        if let Some(last_block) = last_changeset_pruned_block {
            provider
                .static_file_provider()
                .delete_segment_below_block(StaticFileSegment::AccountChangeSets, last_block + 1)?;
        }
        trace!(target: "pruner", pruned = %pruned_changesets, "Pruned account history (changesets from static files)");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        // Sort highest deleted block numbers by account address and turn them into sharded keys.
        // We did not use `BTreeMap` from the beginning, because it's inefficient for hashes.
        let highest_sharded_keys = highest_deleted_accounts
            .into_iter()
            .sorted_unstable() // Unstable is fine because no equal keys exist in the map
            .map(|(address, block_number)| {
                ShardedKey::new(address, block_number.min(last_changeset_pruned_block))
            });
        let outcomes = prune_history_indices::<Provider, tables::AccountsHistory, _>(
            provider,
            highest_sharded_keys,
            |a, b| a.key == b.key,
        )?;
        trace!(target: "pruner", ?outcomes, "Pruned account history (indices)");

        Ok(SegmentOutput {
            progress: limiter.progress(true),
            pruned: pruned_changesets + outcomes.deleted,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }

    fn prune_database<Provider>(
        &self,
        provider: &Provider,
        range: std::ops::RangeInclusive<alloy_primitives::BlockNumber>,
        range_end: alloy_primitives::BlockNumber,
        limiter: crate::segments::PruneLimiter,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider<Tx: DbTxMut>,
    {
        // Number of account history tables to prune in one step.
        // Account History consists of two tables: AccountChangeSets and AccountsHistory.
        // We want to prune them to the same block number.
        const ACCOUNT_HISTORY_TABLES_TO_PRUNE: usize = 2;

        let mut limiter = if let Some(limit) = limiter.deleted_entries_limit() {
            limiter.set_deleted_entries_limit(limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE)
        } else {
            limiter
        };

        let mut last_changeset_pruned_block = None;
        let mut highest_deleted_accounts = FxHashMap::default();
        let (pruned_changesets, done) =
            provider.tx_ref().prune_table_with_range::<tables::AccountChangeSets>(
                range,
                &mut limiter,
                |_| false,
                |(block_number, account)| {
                    highest_deleted_accounts.insert(account.address, block_number);
                    last_changeset_pruned_block = Some(block_number);
                },
            )?;
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned account history (changesets from database)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more account changesets to prune, set the checkpoint block number to
            // previous, so we could finish pruning its account changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        // Sort highest deleted block numbers by account address and turn them into sharded keys.
        // We did not use `BTreeMap` from the beginning, because it's inefficient for hashes.
        let highest_sharded_keys = highest_deleted_accounts
            .into_iter()
            .sorted_unstable() // Unstable is fine because no equal keys exist in the map
            .map(|(address, block_number)| {
                ShardedKey::new(address, block_number.min(last_changeset_pruned_block))
            });
        let outcomes = prune_history_indices::<Provider, tables::AccountsHistory, _>(
            provider,
            highest_sharded_keys,
            |a, b| a.key == b.key,
        )?;
        trace!(target: "pruner", ?outcomes, %done, "Pruned account history (indices)");

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: pruned_changesets + outcomes.deleted,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}
