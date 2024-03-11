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

        // If there's more storage storage changesets to prune, set the checkpoint block number
        // to previous, so we could finish pruning its storage changesets on the next run.
        let last_pruned_block = last_pruned_block
            .map(|block| block.checked_sub(if progress.is_done() { 0 } else { 1 }))
            .flatten();

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

        match provider.with_walker::<tables::StorageChangeSets, _, _>(
            block_step,
            |ref mut walker| {
                provider.step_prune_range(walker, limiter, &mut |_| false, &mut |row| {
                    last_pruned_block_changesets = Some(row.0.block_number())
                })
            },
        ) {
            Err(err) => return Some(Err(err.into())),
            Ok(res) if !res.is_done() => {
                _ = limiter.last_pruned_block().map(|block| block.saturating_sub(1))
            }
            _ => (),
        }

        if limiter.last_pruned_block() < last_pruned_block_changesets {
            limiter.increment_last_pruned_block();
        }

        if let Err(err) = provider.with_cursor::<tables::StoragesHistory, _, _>(|ref mut cursor| {
            step_prune_indices::<DB, _, _>(
                cursor,
                limiter,
                &|a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
                &|key| StorageShardedKey::last(key.address, key.sharded_key.key),
            )
        }) {
            return Some(Err(err.into()))
        }

        if next_up_last_pruned_block != last_pruned_block_changesets {
            return Some(Err(PrunerError::InconsistentData(
                "All headers-related tables should be pruned up to the same height",
            )))
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

    use assert_matches::assert_matches;
    use reth_db::{
        models::storage_sharded_key::StorageShardedKey, tables, BlockNumberList, DatabaseEnv,
    };
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
    use reth_tracing;
    use tracing::trace;

    use crate::segments::{PruneInput, PruneOutput, Segment, StorageHistory};

    #[derive(Default)]
    struct TestRig {
        db: TestStageDB,
        original_shards: Vec<(StorageShardedKey, IntegerList)>,
        changesets: Vec<Vec<(Address, Account, Vec<StorageEntry>)>>,
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

            Self { db, original_shards, changesets }
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
    }

    #[test]
    fn prune() {
        let test_rig = TestRig::new();

        fn test_prune(
            test_rig: &TestRig,
            to_block: BlockNumber,
            run: usize,
            expected_result: (PruneProgress, usize),
        ) {
            reth_tracing::init_test_tracing();

            let prune_mode = PruneMode::Before(to_block);
            let segment = StorageHistory::new(prune_mode);
            let job_limiter = PruneLimiterBuilder::default().deleted_entries_limit(1000).build();
            let limiter =
                <StorageHistory as Segment<DatabaseEnv>>::new_limiter_from_parent_scope_limiter(
                    &segment,
                    &job_limiter,
                );
            let input = test_rig.get_input(to_block, limiter);

            let provider = test_rig.db.factory.provider_rw().unwrap();
            let result = segment.prune(&provider, input.clone()).unwrap();
            trace!(target: "pruner::test",
                expected_prune_progress=?expected_result.0,
                expected_pruned=?expected_result.1,
                result=?result,
                "PruneOutput"
            );
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

            let changesets = test_rig
                .changesets
                .iter()
                .enumerate()
                .flat_map(|(block_number, changeset)| {
                    changeset.iter().flat_map(move |(address, _, entries)| {
                        entries.iter().map(move |entry| (block_number, address, entry))
                    })
                })
                .collect::<Vec<_>>();

            #[allow(clippy::skip_while_next)]
            let pruned = changesets
                .iter()
                .enumerate()
                .skip_while(|(i, (block_number, _, _))| {
                    if let Some(limit) = input.limiter.deleted_entries_limit() {
                        if *i >= limit / 2 * run {
                            return false
                        }
                    }
                    *block_number <= to_block as usize
                })
                .next()
                .map(|(i, _)| i)
                .unwrap_or_default();

            let mut pruned_changesets = changesets
                .iter()
                // Skip what we've pruned so far, subtracting one to get last pruned block number
                // further down
                .skip(pruned.saturating_sub(1));

            let last_pruned_block_number = pruned_changesets
            .next()
            .map(|(block_number, _, _)| if result.progress.is_done() {
                *block_number
            } else {
                block_number.saturating_sub(1)
            } as BlockNumber)
            .unwrap_or(to_block);

            let pruned_changesets = pruned_changesets.fold(
                BTreeMap::<_, Vec<_>>::new(),
                |mut acc, (block_number, address, entry)| {
                    acc.entry((block_number, address)).or_default().push(entry);
                    acc
                },
            );

            assert_eq!(
                test_rig.db.table::<tables::StorageChangeSets>().unwrap().len(),
                pruned_changesets.values().flatten().count()
            );

            let actual_shards = test_rig.db.table::<tables::StoragesHistory>().unwrap();

            let expected_shards = test_rig
                .original_shards
                .iter()
                .filter(|(key, _)| key.sharded_key.highest_block_number > last_pruned_block_number)
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
                test_rig
                    .db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::StorageHistory)
                    .unwrap(),
                Some(PruneCheckpoint {
                    block_number: Some(last_pruned_block_number),
                    tx_number: None,
                    prune_mode
                })
            );
        }

        test_prune(&test_rig, 998, 1, (PruneProgress::new_entries_limit_reached(), 500));
        test_prune(&test_rig, 998, 2, (PruneProgress::new_finished(), 499));
        test_prune(&test_rig, 1200, 3, (PruneProgress::new_finished(), 202));
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
