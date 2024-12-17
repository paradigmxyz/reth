use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment},
    PruneLimiter, PrunerError,
};
use alloy_primitives::BlockNumber;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, RangeWalker},
    tables,
    transaction::DbTxMut,
};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileProvider, DBProvider, NodePrimitivesProvider, StaticFileProviderFactory,
};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use std::num::NonZeroUsize;
use tracing::trace;

/// Number of tables to prune in one step
const TABLES_TO_PRUNE: usize = 3;

#[derive(Debug)]
pub struct BlockMeta<N> {
    static_file_provider: StaticFileProvider<N>,
}

impl<N> BlockMeta<N> {
    pub const fn new(static_file_provider: StaticFileProvider<N>) -> Self {
        Self { static_file_provider }
    }
}

impl<Provider: StaticFileProviderFactory + DBProvider<Tx: DbTxMut>> Segment<Provider>
    for BlockMeta<Provider::Primitives>
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::BlockMeta
    }

    fn mode(&self) -> Option<PruneMode> {
        self.static_file_provider
            .get_highest_static_file_block(StaticFileSegment::BlockMeta)
            .map(PruneMode::before_inclusive)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::StaticFile
    }

    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let (block_range_start, block_range_end) = match input.get_next_block_range() {
            Some(range) => (*range.start(), *range.end()),
            None => {
                trace!(target: "pruner", "No headers to prune");
                return Ok(SegmentOutput::done())
            }
        };

        let last_pruned_block =
            if block_range_start == 0 { None } else { Some(block_range_start - 1) };

        let range = last_pruned_block.map_or(0, |block| block + 1)..=block_range_end;

        let mut indices_cursor = provider.tx_ref().cursor_write::<tables::BlockBodyIndices>()?;
        let ommers_cursor =
            provider.tx_ref().cursor_write::<tables::BlockOmmers<<Provider::Primitives as NodePrimitives>::BlockHeader>>()?;
        let withdrawals_cursor = provider.tx_ref().cursor_write::<tables::BlockWithdrawals>()?;

        let mut limiter = input.limiter.floor_deleted_entries_limit_to_multiple_of(
            NonZeroUsize::new(TABLES_TO_PRUNE).unwrap(),
        );

        let tables_iter = BlockMetaTablesIter::new(
            provider,
            &mut limiter,
            indices_cursor.walk_range(range.clone())?,
            ommers_cursor,
            withdrawals_cursor,
        );

        let mut last_pruned_block: Option<u64> = None;
        let mut pruned = 0;
        for res in tables_iter {
            let BlockMetaTablesIterItem { pruned_block, entries_pruned } = res?;
            last_pruned_block = Some(pruned_block);
            pruned += entries_pruned;
        }

        let done = last_pruned_block == Some(block_range_end);
        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: None,
            }),
        })
    }
}
type Walker<'a, Provider, T> =
    RangeWalker<'a, T, <<Provider as DBProvider>::Tx as DbTxMut>::CursorMut<T>>;

#[allow(missing_debug_implementations)]
struct BlockMetaTablesIter<'a, Provider, C1, C2>
where
    Provider: NodePrimitivesProvider + DBProvider<Tx: DbTxMut>,
{
    provider: &'a Provider,
    limiter: &'a mut PruneLimiter,
    indices_walker: Walker<'a, Provider, tables::BlockBodyIndices>,
    ommers_cursor: C1,
    withdrawals_cursor: C2,
}

struct BlockMetaTablesIterItem {
    pruned_block: BlockNumber,
    entries_pruned: usize,
}

impl<'a, Provider, C1, C2> BlockMetaTablesIter<'a, Provider, C1, C2>
where
    Provider: NodePrimitivesProvider + DBProvider<Tx: DbTxMut>,
{
    fn new(
        provider: &'a Provider,
        limiter: &'a mut PruneLimiter,
        indices_cursor: Walker<'a, Provider, tables::BlockBodyIndices>,
        ommers_cursor: C1,
        withdrawals_cursor: C2,
    ) -> Self {
        Self {
            provider,
            limiter,
            indices_walker: indices_cursor,
            ommers_cursor,
            withdrawals_cursor,
        }
    }
}

impl<Provider, C1, C2> Iterator for BlockMetaTablesIter<'_, Provider, C1, C2>
where
    Provider: NodePrimitivesProvider + DBProvider<Tx: DbTxMut>,
    C1: DbCursorRW<tables::BlockOmmers<<Provider::Primitives as NodePrimitives>::BlockHeader>>
        + DbCursorRO<tables::BlockOmmers<<Provider::Primitives as NodePrimitives>::BlockHeader>>,
    C2: DbCursorRW<tables::BlockWithdrawals> + DbCursorRO<tables::BlockWithdrawals>,
{
    type Item = Result<BlockMetaTablesIterItem, PrunerError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.limiter.is_limit_reached() {
            return None
        }

        let mut entries_pruned = 0;
        let mut pruned_block_indices = None;

        if let Err(err) = self.provider.tx_ref().prune_table_with_range_step(
            &mut self.indices_walker,
            self.limiter,
            &mut |_| false,
            &mut |(block, _)| {
                pruned_block_indices = Some(block);
            },
        ) {
            return Some(Err(err.into()))
        }

        if let Some(block) = &pruned_block_indices.clone() {
            entries_pruned += 1;
            match self.ommers_cursor.seek_exact(*block) {
                Ok(v) if v.is_some() => {
                    if let Err(err) = self.ommers_cursor.delete_current() {
                        return Some(Err(err.into()))
                    }
                    entries_pruned += 1;
                    self.limiter.increment_deleted_entries_count();
                }
                Err(err) => return Some(Err(err.into())),
                Ok(_) => {}
            };
        }

        if let Some(block) = &pruned_block_indices.clone() {
            match self.withdrawals_cursor.seek_exact(*block) {
                Ok(v) if v.is_some() => {
                    if let Err(err) = self.withdrawals_cursor.delete_current() {
                        return Some(Err(err.into()))
                    }
                    entries_pruned += 1;
                    self.limiter.increment_deleted_entries_count();
                }
                Err(err) => return Some(Err(err.into())),
                Ok(_) => {}
            };
        }

        pruned_block_indices
            .map(move |block| Ok(BlockMetaTablesIterItem { pruned_block: block, entries_pruned }))
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, PruneLimiter, Segment, SegmentOutput};
    use alloy_eips::eip4895::{Withdrawal, Withdrawals};
    use alloy_primitives::{BlockNumber, B256};
    use assert_matches::assert_matches;
    use reth_db::{
        models::{StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals},
        tables,
        transaction::DbTxMut,
    };
    use reth_db_api::transaction::DbTx;
    use reth_provider::{
        DatabaseProviderFactory, PruneCheckpointReader, PruneCheckpointWriter,
        StaticFileProviderFactory,
    };
    use reth_prune_types::{
        PruneInterruptReason, PruneMode, PruneProgress, PruneSegment, SegmentOutputCheckpoint,
    };
    use reth_stages::test_utils::TestStageDB;
    use reth_testing_utils::{generators, generators::random_header_range};
    use tracing::trace;

    #[test]
    fn prune() {
        reth_tracing::init_test_tracing();

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let headers = random_header_range(&mut rng, 0..100, B256::ZERO);
        let tx = db.factory.provider_rw().unwrap().into_tx();

        for header in &headers {
            // One tx per block
            tx.put::<tables::BlockBodyIndices>(
                header.number,
                StoredBlockBodyIndices {
                    first_tx_num: header.number.saturating_sub(1),
                    tx_count: 1,
                },
            )
            .unwrap();

            // Not all blocks have ommers,
            if header.number % 2 == 0 {
                tx.put::<tables::BlockOmmers>(
                    header.number,
                    StoredBlockOmmers { ommers: vec![header.clone_header()] },
                )
                .unwrap();
            }

            // Not all blocks have withdrawals
            if header.number % 3 == 0 {
                tx.put::<tables::BlockWithdrawals>(
                    header.number,
                    StoredBlockWithdrawals {
                        withdrawals: Withdrawals::new(vec![Withdrawal::default()]),
                    },
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();

        let initial_ommer_entries = headers.len() / 2;
        let initial_withdrawal_entries = headers.len() / 3 + 1;

        assert_eq!(db.table::<tables::BlockBodyIndices>().unwrap().len(), headers.len());
        // We have only inserted every 2 blocks
        assert_eq!(db.table::<tables::BlockOmmers>().unwrap().len(), initial_ommer_entries);
        // We have only inserted every 3 blocks
        assert_eq!(
            db.table::<tables::BlockWithdrawals>().unwrap().len(),
            initial_withdrawal_entries
        );

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let segment = super::BlockMeta::new(db.factory.static_file_provider());
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::BlockMeta)
                    .unwrap(),
                to_block,
                limiter: PruneLimiter::default().set_deleted_entries_limit(5),
            };

            let provider = db.factory.database_provider_rw().unwrap();
            let result = segment.prune(&provider, input.clone()).unwrap();

            trace!(target: "pruner::test",
                expected_prune_progress=?expected_result.0,
                expected_pruned=?expected_result.1,
                result=?result,
                "SegmentOutput"
            );

            assert_matches!(
                result,
                SegmentOutput {progress, pruned, checkpoint: Some(_)}
                    if (progress, pruned) == expected_result
            );

            provider
                .save_prune_checkpoint(
                    PruneSegment::BlockMeta,
                    result.checkpoint.unwrap().as_prune_checkpoint(PruneMode::Before(to_block + 1)),
                )
                .unwrap();
            provider.commit().expect("commit");
        };

        test_prune(
            3,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 3),
        );
        test_prune(
            3,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 3),
        );
        test_prune(3, (PruneProgress::Finished, 2));
    }

    #[test]
    fn prune_cannot_be_done() {
        let db = TestStageDB::default();

        let limiter = PruneLimiter::default().set_deleted_entries_limit(0);

        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 1,
            // Less than total number of tables for `BlockMeta` segment
            limiter,
        };

        let provider = db.factory.database_provider_rw().unwrap();
        let segment = super::BlockMeta::new(db.factory.static_file_provider());
        let result = segment.prune(&provider, input).unwrap();
        assert_eq!(
            result,
            SegmentOutput::not_done(
                PruneInterruptReason::DeletedEntriesLimitReached,
                Some(SegmentOutputCheckpoint::default())
            )
        );
    }
}
