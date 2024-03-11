use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use itertools::Itertools;
use reth_db::{database::Database, tables};

use reth_primitives::{PruneInterruptReason, PruneMode, PruneProgress, PruneSegment};
use reth_provider::{DatabaseProviderRW, PruneLimiter, PruneLimiterBuilder};
use std::num::NonZeroUsize;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct Headers {
    mode: PruneMode,
}

impl Headers {
    pub fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for Headers {
    fn segment(&self) -> PruneSegment {
        PruneSegment::Headers
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

        let mut limiter = input.limiter;
        // todo: move this check to prune job start where limiter is made and update tests
        // accordingly
        if limiter.is_limit_reached() {
            // Nothing to do, `input.delete_limit` is less than 3 so we can't prune all
            // headers-related tables up to the same height
            return Ok(PruneOutput::not_done(PruneInterruptReason::LimitEntriesDeleted))
        }

        let last_pruned_block =
            if block_range_start == 0 { None } else { Some(block_range_start - 1) };

        let tables_iter =
            HeaderTablesIter::new(provider, &mut limiter, last_pruned_block, block_range_end);

        let mut last_pruned_block: Option<u64> = None;
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
        // limit on entries must be multiple of three to prune headers, headers td and canonical
        // headers tables up to the same height
        PruneLimiterBuilder::floor_deleted_entries_limit_to_multiple_of(
            limiter,
            NonZeroUsize::new(3).unwrap(),
        )
        .build()
    }
}

#[allow(missing_debug_implementations)]
struct HeaderTablesIter<'a, 'b, DB>
where
    DB: Database,
{
    provider: &'a DatabaseProviderRW<DB>,
    limiter: &'b mut PruneLimiter,
    last_pruned_block: Option<u64>,
    to_block: u64,
}

impl<'a, 'b, DB> HeaderTablesIter<'a, 'b, DB>
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

impl<'a, 'b, DB> Iterator for HeaderTablesIter<'a, 'b, DB>
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
        let mut last_pruned_block_headers = None;
        let mut last_pruned_block_td = None;
        let mut last_pruned_block_canonical = None;
        // todo: guarantee skip filter and delete callback are same for all header table types

        if let Err(err) =
            provider.with_walker::<tables::Headers, _, _>(block_step.clone(), |ref mut walker| {
                provider.step_prune_range(walker, limiter, &mut |_| false, &mut |row| {
                    last_pruned_block_headers = Some(row.0)
                })
            })
        {
            return Some(Err(err.into()))
        }

        if let Err(err) = provider.with_walker::<tables::HeaderTerminalDifficulties, _, _>(
            block_step.clone(),
            |ref mut walker| {
                provider.step_prune_range(walker, limiter, &mut |_| false, &mut |row| {
                    last_pruned_block_td = Some(row.0)
                })
            },
        ) {
            return Some(Err(err.into()))
        }

        if let Err(err) =
            provider.with_walker::<tables::CanonicalHeaders, _, _>(block_step, |ref mut walker| {
                provider.step_prune_range(walker, limiter, &mut |_| false, &mut |row| {
                    last_pruned_block_canonical = Some(row.0)
                })
            })
        {
            return Some(Err(err.into()))
        }

        if ![
            next_up_last_pruned_block,
            last_pruned_block_headers,
            last_pruned_block_td,
            last_pruned_block_canonical,
        ]
        .iter()
        .all_equal()
        {
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
    use assert_matches::assert_matches;
    use reth_db::{tables, transaction::DbTx, DatabaseEnv};
    use reth_interfaces::test_utils::{generators, generators::random_header_range};
    use reth_primitives::{
        BlockNumber, PruneCheckpoint, PruneInterruptReason, PruneMode, PruneProgress, PruneSegment,
        B256, U256,
    };
    use reth_provider::{PruneCheckpointReader, PruneLimiterBuilder};
    use reth_stages::test_utils::TestStageDB;
    use reth_tracing;
    use tracing::trace;

    use crate::segments::{Headers, PruneInput, PruneOutput, Segment};

    #[test]
    fn prune() {
        reth_tracing::init_test_tracing();

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let headers = random_header_range(&mut rng, 0..100, B256::ZERO);
        let tx = db.factory.provider_rw().unwrap().into_tx();
        for header in headers.iter() {
            TestStageDB::insert_header(None, &tx, header, U256::ZERO).unwrap();
        }
        tx.commit().unwrap();

        assert_eq!(db.table::<tables::CanonicalHeaders>().unwrap().len(), headers.len());
        assert_eq!(db.table::<tables::Headers>().unwrap().len(), headers.len());
        assert_eq!(db.table::<tables::HeaderTerminalDifficulties>().unwrap().len(), headers.len());

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let segment = Headers::new(prune_mode);
            let job_limiter = PruneLimiterBuilder::default().deleted_entries_limit(10).build();
            let limiter = <Headers as Segment<DatabaseEnv>>::new_limiter_from_parent_scope_limiter(
                &segment,
                &job_limiter,
            );

            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Headers)
                    .unwrap(),
                to_block,
                limiter,
            };

            let next_block_number_to_prune = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::Headers)
                .unwrap()
                .and_then(|checkpoint| checkpoint.block_number)
                .map(|block_number| block_number + 1)
                .unwrap_or_default();

            let provider = db.factory.provider_rw().unwrap();
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

            let last_pruned_block_number = to_block.min(
                next_block_number_to_prune +
                    input.limiter.deleted_entries_limit().unwrap() as BlockNumber / 3 -
                    1,
            );

            assert_eq!(
                db.table::<tables::CanonicalHeaders>().unwrap().len(),
                headers.len() - (last_pruned_block_number + 1) as usize
            );
            assert_eq!(
                db.table::<tables::Headers>().unwrap().len(),
                headers.len() - (last_pruned_block_number + 1) as usize
            );
            assert_eq!(
                db.table::<tables::HeaderTerminalDifficulties>().unwrap().len(),
                headers.len() - (last_pruned_block_number + 1) as usize
            );
            assert_eq!(
                db.factory.provider().unwrap().get_prune_checkpoint(PruneSegment::Headers).unwrap(),
                Some(PruneCheckpoint {
                    block_number: Some(last_pruned_block_number),
                    tx_number: None,
                    prune_mode
                })
            );
        };

        test_prune(3, (PruneProgress::new_entries_limit_reached(), 9));
        test_prune(3, (PruneProgress::new_finished(), 3));
    }

    #[test]
    fn prune_cannot_be_done() {
        let db = TestStageDB::default();

        let segment = Headers::new(PruneMode::Full);
        let job_limiter = PruneLimiterBuilder::default().deleted_entries_limit(0).build();
        let limiter = <Headers as Segment<DatabaseEnv>>::new_limiter_from_parent_scope_limiter(
            &segment,
            &job_limiter,
        );

        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 1,
            // Less than total number of tables for `Headers` segment
            limiter,
        };

        let provider = db.factory.provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        assert_eq!(result, PruneOutput::not_done(PruneInterruptReason::LimitEntriesDeleted));
    }
}
