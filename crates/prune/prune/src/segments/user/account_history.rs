use crate::{
    db_ext::DbTxPruneExt,
    segments::{user::history::prune_history_indices, PruneInput, Segment},
    PrunerError,
};
use itertools::Itertools;
use reth_db::{tables, transaction::DbTxMut};
use reth_db_api::models::ShardedKey;
use reth_provider::DBProvider;
use reth_prune_types::{
    PruneInterruptReason, PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutput,
    SegmentOutputCheckpoint,
};
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

/// Number of account history tables to prune in one step.
///
/// Account History consists of two tables: [`tables::AccountChangeSets`] and
/// [`tables::AccountsHistory`]. We want to prune them to the same block number.
const ACCOUNT_HISTORY_TABLES_TO_PRUNE: usize = 2;

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
    Provider: DBProvider<Tx: DbTxMut>,
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

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No account history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            input.limiter.set_deleted_entries_limit(limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE)
        } else {
            input.limiter
        };
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                PruneInterruptReason::new(&limiter),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut last_changeset_pruned_block = None;
        // Deleted account changeset keys (account addresses) with the highest block number deleted
        // for that key.
        //
        // The size of this map it's limited by `prune_delete_limit * blocks_since_last_run /
        // ACCOUNT_HISTORY_TABLES_TO_PRUNE`, and with current default it's usually `3500 * 5
        // / 2`, so 8750 entries. Each entry is `160 bit + 256 bit + 64 bit`, so the total
        // size should be up to 0.5MB + some hashmap overhead. `blocks_since_last_run` is
        // additionally limited by the `max_reorg_depth`, so no OOM is expected here.
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
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned account history (changesets)");

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

        let progress = PruneProgress::new(done, &limiter);

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

#[cfg(test)]
mod tests {
    use crate::segments::{
        user::account_history::ACCOUNT_HISTORY_TABLES_TO_PRUNE, AccountHistory, PruneInput,
        Segment, SegmentOutput,
    };
    use alloy_primitives::{BlockNumber, B256};
    use assert_matches::assert_matches;
    use reth_db::{tables, BlockNumberList};
    use reth_provider::{DatabaseProviderFactory, PruneCheckpointReader};
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneProgress, PruneSegment,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_eoa_accounts, BlockRangeParams,
    };
    use std::{collections::BTreeMap, ops::AddAssign};

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=5000,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let accounts = random_eoa_accounts(&mut rng, 2).into_iter().collect::<BTreeMap<_, _>>();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            0..0,
            0..0,
        );
        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        let account_occurrences = db.table::<tables::AccountsHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry(key.key).or_default().add_assign(1);
                map
            },
        );
        assert!(account_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            db.table::<tables::AccountChangeSets>().unwrap().len(),
            changesets.iter().flatten().count()
        );

        let original_shards = db.table::<tables::AccountsHistory>().unwrap();

        let test_prune =
            |to_block: BlockNumber, run: usize, expected_result: (PruneProgress, usize)| {
                let prune_mode = PruneMode::Before(to_block);
                let deleted_entries_limit = 2000;
                let mut limiter =
                    PruneLimiter::default().set_deleted_entries_limit(deleted_entries_limit);
                let input = PruneInput {
                    previous_checkpoint: db
                        .factory
                        .provider()
                        .unwrap()
                        .get_prune_checkpoint(PruneSegment::AccountHistory)
                        .unwrap(),
                    to_block,
                    limiter: limiter.clone(),
                };
                let segment = AccountHistory::new(prune_mode);

                let provider = db.factory.database_provider_rw().unwrap();
                let result = segment.prune(&provider, input).unwrap();
                limiter.increment_deleted_entries_count_by(result.pruned);

                assert_matches!(
                    result,
                    SegmentOutput {progress, pruned, checkpoint: Some(_)}
                        if (progress, pruned) == expected_result
                );

                segment
                    .save_checkpoint(
                        &provider,
                        result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                    )
                    .unwrap();
                provider.commit().expect("commit");

                let changesets = changesets
                    .iter()
                    .enumerate()
                    .flat_map(|(block_number, changeset)| {
                        changeset.iter().map(move |change| (block_number, change))
                    })
                    .collect::<Vec<_>>();

                #[allow(clippy::skip_while_next)]
                let pruned = changesets
                    .iter()
                    .enumerate()
                    .skip_while(|(i, (block_number, _))| {
                        *i < deleted_entries_limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE * run &&
                            *block_number <= to_block as usize
                    })
                    .next()
                    .map(|(i, _)| i)
                    .unwrap_or_default();

                let mut pruned_changesets = changesets
                    .iter()
                    // Skip what we've pruned so far, subtracting one to get last pruned block
                    // number further down
                    .skip(pruned.saturating_sub(1));

                let last_pruned_block_number = pruned_changesets
                .next()
                .map(|(block_number, _)| if result.progress.is_finished() {
                    *block_number
                } else {
                    block_number.saturating_sub(1)
                } as BlockNumber)
                .unwrap_or(to_block);

                let pruned_changesets = pruned_changesets.fold(
                    BTreeMap::<_, Vec<_>>::new(),
                    |mut acc, (block_number, change)| {
                        acc.entry(block_number).or_default().push(change);
                        acc
                    },
                );

                assert_eq!(
                    db.table::<tables::AccountChangeSets>().unwrap().len(),
                    pruned_changesets.values().flatten().count()
                );

                let actual_shards = db.table::<tables::AccountsHistory>().unwrap();

                let expected_shards = original_shards
                    .iter()
                    .filter(|(key, _)| key.highest_block_number > last_pruned_block_number)
                    .map(|(key, blocks)| {
                        let new_blocks =
                            blocks.iter().skip_while(|block| *block <= last_pruned_block_number);
                        (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                    })
                    .collect::<Vec<_>>();

                assert_eq!(actual_shards, expected_shards);

                assert_eq!(
                    db.factory
                        .provider()
                        .unwrap()
                        .get_prune_checkpoint(PruneSegment::AccountHistory)
                        .unwrap(),
                    Some(PruneCheckpoint {
                        block_number: Some(last_pruned_block_number),
                        tx_number: None,
                        prune_mode
                    })
                );
            };

        test_prune(
            998,
            1,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 1000),
        );
        test_prune(998, 2, (PruneProgress::Finished, 998));
        test_prune(1400, 3, (PruneProgress::Finished, 804));
    }
}
