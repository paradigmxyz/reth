use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use reth_db::{database::Database, models::ShardedKey, tables};
use reth_primitives::{PruneInterruptReason, PruneMode, PruneProgress, PruneSegment};
use reth_provider::{DatabaseProviderRW, PruneLimiter, PruneLimiterBuilder};
use tracing::{instrument, trace};

/// Number of account history tables to prune in one step.
///
/// Account History consists of two tables: [tables::AccountChangeSets] and
/// [tables::AccountsHistory]. We want to prune them to the same block number.
const ACCOUNT_HISTORY_TABLES_TO_PRUNE: usize = 2;

#[derive(Debug)]
pub struct AccountHistory {
    mode: PruneMode,
}

impl AccountHistory {
    pub fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for AccountHistory {
    fn segment(&self) -> PruneSegment {
        PruneSegment::AccountHistory
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError> {
        let (block_range_start, block_range_end) = match input.get_next_block_range() {
            Some(range) => (*range.start(), *range.end()),
            None => {
                trace!(target: "pruner", "No headers to prune");
                return Ok(PruneOutput::done())
            }
        };
        let range_end = *range.end();

        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            input.limiter.set_deleted_entries_limit(limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE)
        } else {
            input.limiter
        };
        if limiter.is_limit_reached() {
            return Ok(PruneOutput::not_done(
                PruneInterruptReason::new(&limiter),
                input.previous_checkpoint.map(|checkpoint| checkpoint.into()),
            ))
        }

        let mut last_changeset_pruned_block = None;
        let (pruned_changesets, done) = provider
            .prune_table_with_range::<tables::AccountChangeSets>(
                range,
                &mut limiter,
                |_| false,
                |row| last_changeset_pruned_block = Some(row.0),
            )?;
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned account history (changesets)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more account account changesets to prune, set the checkpoint block number
            // to previous, so we could finish pruning its account changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let tables_iter = AccountHistoryTablesIter::new(
            provider,
            &mut limiter,
            last_pruned_block,
            block_range_end,
        );

        for res in tables_iter.into_iter() {
            last_pruned_block = res?;
        }

        let done = || -> bool {
            let Some(block) = last_pruned_block else { return false };
            block == block_range_end
        }();

        trace!(target: "pruner", %processed, pruned = %pruned_indices, %done, "Pruned account history (history)");

        let progress = PruneProgress::new(done, &limiter);

        Ok(PruneOutput {
            progress,
            pruned,
            checkpoint: Some(PruneOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: None,
            }),
        })
    }
}

#[allow(missing_debug_implementations)]
struct AccountHistoryTablesIter<'a, 'b, DB>
where
    DB: Database,
{
    provider: &'a DatabaseProviderRW<DB>,
    limiter: &'b mut PruneLimiter,
    last_pruned_block: Option<u64>,
    to_block: u64,
}

impl<'a, 'b, DB> AccountHistoryTablesIter<'a, 'b, DB>
where
    DB: Database,
{
    fn new(
        provider: &'a DatabaseProviderRW<DB>,
        limiter: &'b mut PruneLimiter,
        last_pruned_block: Option<u64>,
        to_block: u64,
    ) -> Self {
        Self { provider, limiter, last_pruned_block, to_block }
    }
}

impl<'a, 'b, DB> Iterator for AccountHistoryTablesIter<'a, 'b, DB>
where
    DB: Database,
{
    type Item = Result<Option<u64>, PrunerError>;
    fn next(&mut self) -> Option<Self::Item> {
        let Self { provider, limiter, last_pruned_block, to_block } = self;

        if limiter.is_limit_reached() || Some(*to_block) == *last_pruned_block {
            return None
        }

        let block_step =
            if let Some(block) = *last_pruned_block { block + 1..=block + 2 } else { 0..=1 };

        let next_up_last_pruned_block = Some(*block_step.start());
        let mut last_pruned_block_changesets = None;
        // todo: guarantee skip filter and delete callback are same for all header table types

        let to_block = match provider.with_walker::<tables::AccountChangeSets, _, _>(
            block_step.clone(),
            |ref mut walker| {
                provider.step_prune_range(walker, limiter, &mut |_| false, &mut |row| {
                    last_pruned_block_changesets = Some(row.0)
                })
            },
        ) {
            Err(err) => return Some(Err(err.into())),
            Ok(res) => if res.is_done() {
                last_pruned_block_changesets
            } else {
                last_pruned_block_changesets.map(|block| block.saturating_sub(1))
            }
            .unwrap_or(*block_step.end()),
        };

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

                let provider = db.factory.provider_rw().unwrap();
                let result = segment.prune(&provider, input).unwrap();
                limiter.increment_deleted_entries_count_by(result.pruned);

                assert_matches!(
                    result,
                    PruneOutput {progress, pruned, checkpoint: Some(_)}
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
                        let new_blocks = blocks
                            .iter()
                            .skip_while(|block| *block <= last_pruned_block_number)
                            .collect::<Vec<_>>();
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
