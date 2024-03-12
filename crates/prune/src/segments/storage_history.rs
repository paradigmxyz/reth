use std::num::NonZeroUsize;

use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use reth_db::{
    database::Database,
    models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress},
    tables,
};
use reth_primitives::{PruneMode, PruneProgress, PruneSegment};
use reth_provider::{DatabaseProviderRW, PruneLimiter, PruneLimiterBuilder};
use tracing::{instrument, trace};

use super::history::step_prune_indices;

#[derive(Debug)]
pub struct StorageHistory {
    mode: PruneMode,
}

impl StorageHistory {
    pub fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for StorageHistory {
    fn segment(&self) -> PruneSegment {
        PruneSegment::StorageHistory
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

        let tables_iter = StorageHistoryTablesIter::new(
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

#[allow(missing_debug_implementations)]
struct StorageHistoryTablesIter<'a, 'b, DB>
where
    DB: Database,
{
    provider: &'a DatabaseProviderRW<DB>,
    limiter: &'b mut PruneLimiter,
    last_pruned_block: Option<u64>,
    to_block: u64,
}

impl<'a, 'b, DB> StorageHistoryTablesIter<'a, 'b, DB>
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

impl<'a, 'b, DB> Iterator for StorageHistoryTablesIter<'a, 'b, DB>
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
            BlockNumberAddress::range_inclusive(if let Some(block) = *last_pruned_block {
                block + 1..=block + 2
            } else {
                0..=1
            });

        let next_up_last_pruned_block = Some(block_step.start().block_number());
        let mut last_pruned_block_changesets = None;
        // todo: guarantee skip filter and delete callback are same for all header table types

        let to_block = match provider.with_walker::<tables::StorageChangeSets, _, _>(
            block_step.clone(),
            |ref mut walker| {
                provider.step_prune_range(walker, limiter, &mut |_| false, &mut |row| {
                    last_pruned_block_changesets = Some(row.0.block_number())
                })
            },
        ) {
            Err(err) => return Some(Err(err.into())),
            Ok(res) => if res.is_done() {
                last_pruned_block_changesets
            } else {
                last_pruned_block_changesets.map(|block| block.saturating_sub(1))
            }
            .unwrap_or(block_step.end().block_number()),
        };

        if let Err(err) = provider.with_cursor::<tables::StoragesHistory, _, _>(|ref mut cursor| {
            step_prune_indices::<DB, _, _>(
                cursor,
                to_block,
                limiter,
                &|a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
                &|key| StorageShardedKey::last(key.address, key.sharded_key.key),
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
        ops::AddAssign,
        thread,
        time::{Duration, Instant},
    };

    use reth_db::{models::storage_sharded_key::StorageShardedKey, tables, DatabaseEnv};
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_changeset_range, random_eoa_accounts},
    };
    use reth_primitives::{
        Account, Address, BlockNumber, IntegerList, PruneCheckpoint, PruneMode, PruneProgress,
        PruneSegment, StorageEntry, B256,
    };
    use reth_provider::{PruneCheckpointReader, PruneLimiter, PruneLimiterBuilder};
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use tracing::trace;

    use crate::segments::{PruneInput, Segment, StorageHistory};

    #[derive(Default)]
    struct TestRig {
        db: TestStageDB,
        original_shards: Vec<(StorageShardedKey, IntegerList)>,
        changesets: Vec<Vec<(Address, Account, Vec<StorageEntry>)>>,
        pruned_changesets_run_1: usize,
        pruned_changesets_run_2: usize,
        pruned_shards_run_1: usize,
        pruned_shards_run_2: usize,
    }

    impl TestRig {
        fn new() -> Self {
            let db = TestStageDB::default();
            let mut rng = generators::rng();

            let blocks = random_block_range(&mut rng, 0..=5000, B256::ZERO, 0..1);
            db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

            let accounts = random_eoa_accounts(&mut rng, 2).into_iter().collect::<BTreeMap<_, _>>();

            let (changesets, _) = random_changeset_range(
                &mut rng,
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                1..2,
                1..2,
            );
            db.insert_changesets(changesets.clone(), None).expect("insert changesets");
            db.insert_history(changesets.clone(), None).expect("insert history");

            let storage_occurrences = db
                .table::<tables::StoragesHistory>()
                .unwrap()
                .into_iter()
                .fold(BTreeMap::<_, usize>::new(), |mut map, (key, _)| {
                    map.entry((key.address, key.sharded_key.key)).or_default().add_assign(1);
                    map
                });
            assert!(storage_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

            assert_eq!(
                db.table::<tables::StorageChangeSets>().unwrap().len(),
                changesets.iter().flatten().flat_map(|(_, _, entries)| entries).count()
            );

            let original_shards = db.table::<tables::StoragesHistory>().unwrap();

            Self { db, original_shards, changesets, ..Default::default() }
        }

        fn get_input(&self, to_block: BlockNumber, limiter: PruneLimiter) -> PruneInput {
            PruneInput {
                previous_checkpoint: self
                    .db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::StorageHistory)
                    .unwrap(),
                to_block,
                limiter,
            }
        }

        fn pruned_changesets(&mut self, run: usize) -> usize {
            // change sets
            let changesets = self.db.table::<tables::StorageChangeSets>().unwrap();
            let pruned_changesets = self.changesets.len() - changesets.len();

            if run == 1 {
                self.pruned_changesets_run_1 = pruned_changesets;

                self.pruned_changesets_run_1
            } else if run == 2 {
                self.pruned_changesets_run_2 = pruned_changesets - self.pruned_changesets_run_1;

                self.pruned_changesets_run_2
            } else if run == 3 {
                pruned_changesets - self.pruned_changesets_run_1 - self.pruned_changesets_run_2
            } else {
                unreachable!()
            }
        }

        fn pruned_shards(&mut self, run: usize) -> usize {
            // shards
            let shards = self.db.table::<tables::StoragesHistory>().unwrap();

            let completely_pruned_shards = self.original_shards.len() - shards.len();
            // branch not covered in test
            assert_eq!(0, completely_pruned_shards);

            let partially_pruned_shards = self.partially_pruned_shards();

            if run == 1 {
                self.pruned_shards_run_1 = partially_pruned_shards;

                self.pruned_shards_run_1
            } else if run == 2 {
                self.pruned_shards_run_2 = partially_pruned_shards - self.pruned_shards_run_1;

                self.pruned_shards_run_2
            } else if run == 3 {
                partially_pruned_shards - self.pruned_shards_run_1 - self.pruned_shards_run_2
            } else {
                unreachable!()
            }
        }

        fn partially_pruned_shards(&self) -> usize {
            let mut partially_pruned_shards = 0;

            for (pruned_shard, original_shard) in self
                .db
                .table::<tables::StoragesHistory>()
                .unwrap()
                .iter()
                .zip(self.original_shards.iter())
            {
                let original_shard_blocks = &original_shard.1;
                let pruned_shard_blocks = &pruned_shard.1;
                if original_shard_blocks != pruned_shard_blocks {
                    let original_shard_blocks_len = original_shard_blocks.len() as usize;
                    let pruned_shard_blocks_len = pruned_shard_blocks.len() as usize;

                    trace!(target: "pruner::test",
                        original_shard_blocks_len,
                        pruned_shard_blocks_len,
                        "partially pruned shard"
                    );

                    // since each step in pruning indices, if partially pruning the shard, makes a
                    // new shard (upserts) with at most one block less than the current one, the
                    // number of partially pruned shards is equal to the number of blocks pruned
                    // from that shard
                    partially_pruned_shards += original_shard_blocks_len - pruned_shard_blocks_len;
                }
            }

            partially_pruned_shards
        }
    }

    fn test_prune_with_entries_delete_limit(
        test_rig: &mut TestRig,
        to_block: BlockNumber,
        run: usize,
        expected_progress: PruneProgress,
    ) {
        reth_tracing::init_test_tracing();

        let prune_mode = PruneMode::Before(to_block);
        let segment = StorageHistory::new(prune_mode);

        let prune_job_limit = 1000;

        let job_limiter =
            PruneLimiterBuilder::default().deleted_entries_limit(prune_job_limit).build();
        let limiter =
            <StorageHistory as Segment<DatabaseEnv>>::new_limiter_from_parent_scope_limiter(
                &segment,
                &job_limiter,
            );

        // expected to be the same since 1000 is a multiple of 2
        assert_eq!(prune_job_limit, limiter.deleted_entries_limit().unwrap());

        let input = test_rig.get_input(to_block, limiter);

        let provider = test_rig.db.factory.provider_rw().unwrap();
        let result = segment.prune(&provider, input.clone()).unwrap();

        assert_eq!(result.progress, expected_progress,);

        if !expected_progress.is_done() {
            assert_eq!(result.pruned, prune_job_limit, "run {run}")
        }

        segment
            .save_checkpoint(&provider, result.checkpoint.unwrap().as_prune_checkpoint(prune_mode))
            .unwrap();
        provider.commit().expect("commit");

        let pruned_changesets = test_rig.pruned_changesets(run);
        let pruned_shards = test_rig.pruned_shards(run);

        trace!(target: "pruner::test",
            pruned_changesets,
            pruned_shards,
            run,
            "total pruned entries in run"
        );

        assert_eq!(pruned_changesets + pruned_shards, result.pruned, "run {run}");

        assert_eq!(
            test_rig
                .db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::StorageHistory)
                .unwrap(),
            Some(PruneCheckpoint {
                block_number: result.checkpoint.unwrap().block_number,
                tx_number: None,
                prune_mode
            })
        );
    }

    #[test]
    fn prune() {
        let mut test_rig = TestRig::new();

        // limit on deleted entries is 1000
        test_prune_with_entries_delete_limit(
            &mut test_rig,
            998,
            1,
            PruneProgress::new_entries_limit_reached(),
        );
        test_prune_with_entries_delete_limit(&mut test_rig, 998, 2, PruneProgress::new_finished());
        test_prune_with_entries_delete_limit(&mut test_rig, 1200, 3, PruneProgress::new_finished());
    }

    #[test]
    fn timeout_prune() {
        const PRUNE_JOB_TIMEOUT: Duration = Duration::from_millis(100);
        const TO_BLOCK: u64 = 1200;
        const PRUNE_MODE: PruneMode = PruneMode::Before(TO_BLOCK);

        let start = Instant::now();

        let limiter = PruneLimiterBuilder::default().job_timeout(PRUNE_JOB_TIMEOUT, start).build();
        let segment = StorageHistory::new(PRUNE_MODE);
        let test_rig = TestRig::new();
        let input = test_rig.get_input(TO_BLOCK, limiter);

        thread::sleep(PRUNE_JOB_TIMEOUT);

        let provider = test_rig.db.factory.provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();

        assert!(result.progress.is_timed_out())
    }
}
