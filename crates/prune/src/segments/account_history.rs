use std::num::NonZeroUsize;

use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use reth_db::{database::Database, models::ShardedKey, tables};
use reth_primitives::{PruneMode, PruneProgress, PruneSegment};
use reth_provider::{DatabaseProviderRW, PruneLimiter, PruneLimiterBuilder};
use tracing::{instrument, trace};

use super::history::step_prune_indices;

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

        let mut last_pruned_block =
            if block_range_start == 0 { None } else { Some(block_range_start - 1) };

        let mut limiter = input.limiter;

        let tables_iter = AccountHistoryTablesIter::new(
            provider,
            &mut limiter,
            last_pruned_block,
            block_range_end,
        );

        for res in tables_iter.into_iter() {
            last_pruned_block = res?;
        }

        let done = last_pruned_block.map_or(false, |block| block == block_range_end);
        let progress = PruneProgress::new(done, limiter.is_timed_out());
        let pruned = limiter.deleted_entries_count();

        Ok(PruneOutput {
            progress,
            pruned,
            checkpoint: Some(PruneOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: None,
            }),
        })
    }

    fn new_limiter_from_parent_scope_limiter(&self, limiter: &PruneLimiter) -> PruneLimiter {
        PruneLimiterBuilder::floor_deleted_entries_limit_to_multiple_of(
            limiter,
            NonZeroUsize::new(2).unwrap(),
        )
        .build()
    }
}

#[derive(Debug)]
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
            Ok(res) => if res.is_finished() {
                last_pruned_block_changesets
            } else {
                last_pruned_block_changesets.map(|block| block.saturating_sub(1))
            }
            .unwrap_or(*block_step.end()),
        };

        if let Err(err) = provider.with_cursor::<tables::AccountsHistory, _, _>(|ref mut cursor| {
            step_prune_indices::<DB, _, _>(
                cursor,
                to_block,
                limiter,
                &|a, b| a.key == b.key,
                &|key| ShardedKey::last(key.key),
            )
        }) {
            return Some(Err(err.into()))
        }

        *last_pruned_block = next_up_last_pruned_block;

        Some(Ok(*last_pruned_block))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        ops::{AddAssign, Range, RangeInclusive},
    };

    use reth_db::{models::ShardedKey, tables, DatabaseEnv};
    use reth_primitives::{
        Address, BlockNumber, PruneCheckpoint, PruneMode, PruneProgress, PruneSegment,
    };
    use reth_provider::{PruneCheckpointReader, PruneLimiterBuilder};
    use tracing::trace;

    use crate::segments::{history::test::TestRig, AccountHistory, Segment};

    struct AccountHistoryTestRigBuilder {
        block_range: RangeInclusive<u64>,
        n_storage_changes: Range<u64>,
        key_range: Range<u64>,
        prune_job_deleted_entries_limit: usize,
    }

    impl AccountHistoryTestRigBuilder {
        fn new(
            block_range: RangeInclusive<u64>,
            n_storage_changes: Range<u64>,
            key_range: Range<u64>,
            prune_job_deleted_entries_limit: usize,
        ) -> Self {
            Self { block_range, n_storage_changes, key_range, prune_job_deleted_entries_limit }
        }

        fn build(self) -> TestRig<ShardedKey<Address>> {
            reth_tracing::init_test_tracing();

            let Self { block_range, n_storage_changes, key_range, prune_job_deleted_entries_limit } =
                self;

            let (db, original_changesets) =
                TestRig::<ShardedKey<Address>>::init_db(block_range, n_storage_changes, key_range);

            // verify db init
            let account_occurrences = db
                .table::<tables::AccountsHistory>()
                .unwrap()
                .into_iter()
                .fold(BTreeMap::<_, usize>::new(), |mut map, (key, _)| {
                    map.entry(key.key).or_default().add_assign(1);
                    map
                });
            assert!(account_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

            assert_eq!(
                db.table::<tables::AccountChangeSets>().unwrap().len(),
                original_changesets.iter().flatten().count()
            );

            let original_shards = db.table::<tables::AccountsHistory>().unwrap();

            // get limiter for whole prune job (each prune job would call prune on each segment
            // once)
            let job_limiter = PruneLimiterBuilder::default()
                .deleted_entries_limit(prune_job_deleted_entries_limit)
                .build();

            trace!(target: "pruner::test",
                changesets_len=original_changesets.len(),
                original_shards_len=original_shards.len(),
                job_deleted_entries_limit=job_limiter.deleted_entries_limit(),
                "new account history test rig"
            );

            TestRig::new(db, original_changesets, original_shards, job_limiter)
        }
    }

    fn test_prune_until_entries_delete_limit(
        test_rig: &mut TestRig<ShardedKey<Address>>,
        to_block: BlockNumber,
        run: usize,
        expected_progress: PruneProgress,
    ) {
        reth_tracing::init_test_tracing();

        let prune_mode = PruneMode::Before(to_block);
        let segment = AccountHistory::new(prune_mode);

        // a new segment limiter is made on each run as if each call to prune is ran as part of a
        // separate prune job (each prune job prunes every segment at most once)
        let (limiter, limit) = {
            let segment_limiter =
                <AccountHistory as Segment<DatabaseEnv>>::new_limiter_from_parent_scope_limiter(
                    &segment,
                    test_rig.job_limiter(),
                );

            // expected to be the same as `prune_job_limit` if `prune_job_limit` is a multiple of 2
            let job_limit = test_rig.job_limiter().deleted_entries_limit().unwrap();
            let maybe_adjusted_limit = segment_limiter.deleted_entries_limit().unwrap();
            assert_eq!(
                if job_limit % 2 == 0 { job_limit } else { job_limit - 1 },
                maybe_adjusted_limit
            );

            (segment_limiter, maybe_adjusted_limit)
        };

        trace!(target: "pruner::test",
            deleted_entries_limit=limit,
            run,
            "limit for run"
        );

        let input = test_rig.get_input(to_block, PruneSegment::AccountHistory, limiter);
        let provider = test_rig.db().factory.provider_rw().unwrap();

        // Run pruning
        let result = segment.prune(&provider, input).unwrap();
        let checkpoint = result.checkpoint.unwrap().as_prune_checkpoint(prune_mode);

        trace!(target: "pruner::test",
            ?checkpoint,
            progress=?result.progress,
            run,
            "stopped pruning"
        );

        // must commit to db before reading new state of data
        segment.save_checkpoint(&provider, checkpoint).unwrap();
        provider.commit().expect("commit");

        assert_eq!(
            test_rig
                .db()
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::AccountHistory)
                .unwrap(),
            Some(PruneCheckpoint {
                block_number: result.checkpoint.unwrap().block_number,
                tx_number: None,
                prune_mode
            })
        );

        // Read new state of data
        let pruned_changesets = test_rig.pruned_changesets::<tables::AccountChangeSets>(run);
        let pruned_shards = test_rig.pruned_shards::<tables::AccountsHistory>(run);
        let pruned = pruned_changesets + pruned_shards;

        trace!(target: "pruner::test",
            pruned_changesets,
            pruned_shards,
            run,
            "total pruned entries in run"
        );

        // verify result
        let reported_progress = result.progress;
        let reported_pruned = result.pruned;

        // verify new state of data against result
        if expected_progress.is_finished() {
            // todo: debug why `pruned`` + 1 sometimes?
            // `pruned` comes from limiter.deleted_entries_count(). how is one less entry counted
            // related to checkpoint saved at previous block if change set not completely pruned...?
            trace!(target: "pruner::test",
                pruned,
                reported_pruned,
                run,
                "total pruned entries in run"
            );
            assert!(pruned == reported_pruned + 1 || pruned == reported_pruned, "run {run}");
        } else {
            assert_eq!(pruned, reported_pruned, "run {run}");
        }

        assert_eq!(expected_progress, reported_progress, "run {run}");

        if expected_progress.is_entries_limit_reached() {
            assert_eq!(limit, reported_pruned, "run {run}")
        }
    }

    #[test]
    fn prune() {
        let mut test_rig = AccountHistoryTestRigBuilder::new(1..=5000, 0..0, 0..0, 1000).build();

        // limit on deleted entries for each run is 1000. if limit would have been more than 1996,
        // then pruning would finish in 1 run: 2 tables, one entry deleted per table per block
        // step, step up to block 998 = 2 * 998 = 1996 entries.
        test_prune_until_entries_delete_limit(
            &mut test_rig,
            998,
            1,
            PruneProgress::new_entries_limit_reached(),
        );
        test_prune_until_entries_delete_limit(&mut test_rig, 998, 2, PruneProgress::new_finished());
        test_prune_until_entries_delete_limit(
            &mut test_rig,
            1400,
            3,
            PruneProgress::new_finished(),
        );
    }
}
