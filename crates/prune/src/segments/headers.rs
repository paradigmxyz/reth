use std::num::NonZeroUsize;

use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, RangeWalker},
    database::Database,
    tables,
    transaction::DbTxMut,
};

use reth_primitives::{BlockNumber, PruneLimiter, PruneMode, PruneProgress, PruneSegment};
use reth_provider::DatabaseProviderRW;
use tracing::{instrument, trace};

/// Number of header tables to prune in one step
const HEADER_TABLES_TO_PRUNE: usize = 3;

#[derive(Debug)]
pub struct Headers {
    mode: PruneMode,
}

impl Headers {
    pub const fn new(mode: PruneMode) -> Self {
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

        let last_pruned_block =
            if block_range_start == 0 { None } else { Some(block_range_start - 1) };

        let range = last_pruned_block.map_or(0, |block| block + 1)..=block_range_end;

        let mut headers_cursor = provider.tx_ref().cursor_write::<tables::Headers>()?;
        let mut header_tds_cursor =
            provider.tx_ref().cursor_write::<tables::HeaderTerminalDifficulties>()?;
        let mut canonical_headers_cursor =
            provider.tx_ref().cursor_write::<tables::CanonicalHeaders>()?;

        let mut limiter = input.limiter.floor_deleted_entries_limit_to_multiple_of(
            NonZeroUsize::new(HEADER_TABLES_TO_PRUNE).unwrap(),
        );

        let tables_iter = HeaderTablesIter::new(
            provider,
            &mut limiter,
            headers_cursor.walk_range(range.clone())?,
            header_tds_cursor.walk_range(range.clone())?,
            canonical_headers_cursor.walk_range(range)?,
        );

        let mut last_pruned_block: Option<u64> = None;
        let mut pruned = 0;
        for res in tables_iter {
            let HeaderTablesIterItem { pruned_block, entries_pruned } = res?;
            last_pruned_block = Some(pruned_block);
            pruned += entries_pruned;
        }

        let done = last_pruned_block.map_or(false, |block| block == block_range_end);
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

type Walker<'a, DB, T> = RangeWalker<'a, T, <<DB as Database>::TXMut as DbTxMut>::CursorMut<T>>;

#[allow(missing_debug_implementations)]
struct HeaderTablesIter<'a, DB>
where
    DB: Database,
{
    provider: &'a DatabaseProviderRW<DB>,
    limiter: &'a mut PruneLimiter,
    headers_walker: Walker<'a, DB, tables::Headers>,
    header_tds_walker: Walker<'a, DB, tables::HeaderTerminalDifficulties>,
    canonical_headers_walker: Walker<'a, DB, tables::CanonicalHeaders>,
}

struct HeaderTablesIterItem {
    pruned_block: BlockNumber,
    entries_pruned: usize,
}

impl<'a, DB> HeaderTablesIter<'a, DB>
where
    DB: Database,
{
    fn new(
        provider: &'a DatabaseProviderRW<DB>,
        limiter: &'a mut PruneLimiter,
        headers_walker: Walker<'a, DB, tables::Headers>,
        header_tds_walker: Walker<'a, DB, tables::HeaderTerminalDifficulties>,
        canonical_headers_walker: Walker<'a, DB, tables::CanonicalHeaders>,
    ) -> Self {
        Self { provider, limiter, headers_walker, header_tds_walker, canonical_headers_walker }
    }
}

impl<'a, DB> Iterator for HeaderTablesIter<'a, DB>
where
    DB: Database,
{
    type Item = Result<HeaderTablesIterItem, PrunerError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.limiter.is_limit_reached() {
            return None
        }

        let mut pruned_block_headers = None;
        let mut pruned_block_td = None;
        let mut pruned_block_canonical = None;

        if let Err(err) = self.provider.prune_table_with_range_step(
            &mut self.headers_walker,
            self.limiter,
            &mut |_| false,
            &mut |row| pruned_block_headers = Some(row.0),
        ) {
            return Some(Err(err.into()))
        }

        if let Err(err) = self.provider.prune_table_with_range_step(
            &mut self.header_tds_walker,
            self.limiter,
            &mut |_| false,
            &mut |row| pruned_block_td = Some(row.0),
        ) {
            return Some(Err(err.into()))
        }

        if let Err(err) = self.provider.prune_table_with_range_step(
            &mut self.canonical_headers_walker,
            self.limiter,
            &mut |_| false,
            &mut |row| pruned_block_canonical = Some(row.0),
        ) {
            return Some(Err(err.into()))
        }

        if ![pruned_block_headers, pruned_block_td, pruned_block_canonical].iter().all_equal() {
            return Some(Err(PrunerError::InconsistentData(
                "All headers-related tables should be pruned up to the same height",
            )))
        }

        pruned_block_headers.map(move |block| {
            Ok(HeaderTablesIterItem { pruned_block: block, entries_pruned: HEADER_TABLES_TO_PRUNE })
        })
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use reth_db::{tables, transaction::DbTx};
    use reth_primitives::{
        BlockNumber, PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneProgress,
        PruneSegment, B256, U256,
    };
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestStageDB;
    use reth_testing_utils::{generators, generators::random_header_range};
    use tracing::trace;

    use crate::segments::{
        headers::HEADER_TABLES_TO_PRUNE, Headers, PruneInput, PruneOutput, PruneOutputCheckpoint,
        Segment,
    };

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
            let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Headers)
                    .unwrap(),
                to_block,
                limiter: limiter.clone(),
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
            limiter.increment_deleted_entries_count_by(result.pruned);
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
                    (input.limiter.deleted_entries_limit().unwrap() / HEADER_TABLES_TO_PRUNE - 1)
                        as u64,
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

        test_prune(
            3,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 9),
        );
        test_prune(3, (PruneProgress::Finished, 3));
    }

    #[test]
    fn prune_cannot_be_done() {
        let db = TestStageDB::default();

        let segment = Headers::new(PruneMode::Full);
        let limiter = PruneLimiter::default().set_deleted_entries_limit(0);

        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 1,
            // Less than total number of tables for `Headers` segment
            limiter,
        };

        let provider = db.factory.provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        assert_eq!(
            result,
            PruneOutput::not_done(
                PruneInterruptReason::DeletedEntriesLimitReached,
                Some(PruneOutputCheckpoint::default())
            )
        );
    }
}
