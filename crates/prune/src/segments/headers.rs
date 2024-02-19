use crate::{
    segments::{PruneInput, PruneOutput, PruneOutputCheckpoint, Segment},
    PrunerError,
};
use itertools::Itertools;
use reth_db::{database::Database, table::Table, tables};
use reth_interfaces::RethResult;
use reth_primitives::{BlockNumber, PruneMode, PruneSegment};
use reth_provider::DatabaseProviderRW;
use std::ops::RangeInclusive;
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
        let block_range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No headers to prune");
                return Ok(PruneOutput::done())
            }
        };

        let delete_limit = input.delete_limit / 3;
        if delete_limit == 0 {
            // Nothing to do, `input.delete_limit` is less than 3, so we can't prune all
            // headers-related tables up to the same height
            return Ok(PruneOutput::not_done())
        }

        let results = [
            self.prune_table::<DB, tables::CanonicalHeaders>(
                provider,
                block_range.clone(),
                delete_limit,
            )?,
            self.prune_table::<DB, tables::Headers>(provider, block_range.clone(), delete_limit)?,
            self.prune_table::<DB, tables::HeaderTD>(provider, block_range, delete_limit)?,
        ];

        if !results.iter().map(|(_, _, last_pruned_block)| last_pruned_block).all_equal() {
            return Err(PrunerError::InconsistentData(
                "All headers-related tables should be pruned up to the same height",
            ))
        }

        let (done, pruned, last_pruned_block) = results.into_iter().fold(
            (true, 0, 0),
            |(total_done, total_pruned, _), (done, pruned, last_pruned_block)| {
                (total_done && done, total_pruned + pruned, last_pruned_block)
            },
        );

        Ok(PruneOutput {
            done,
            pruned,
            checkpoint: Some(PruneOutputCheckpoint {
                block_number: Some(last_pruned_block),
                tx_number: None,
            }),
        })
    }
}

impl Headers {
    /// Prune one headers-related table.
    ///
    /// Returns `done`, number of pruned rows and last pruned block number.
    fn prune_table<DB: Database, T: Table<Key = BlockNumber>>(
        &self,
        provider: &DatabaseProviderRW<DB>,
        range: RangeInclusive<BlockNumber>,
        delete_limit: usize,
    ) -> RethResult<(bool, usize, BlockNumber)> {
        let mut last_pruned_block = *range.end();
        let (pruned, done) = provider.prune_table_with_range::<T>(
            range,
            delete_limit,
            |_| false,
            |row| last_pruned_block = row.0,
        )?;
        trace!(target: "pruner", %pruned, %done, table = %T::TABLE, "Pruned headers");

        Ok((done, pruned, last_pruned_block))
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{Headers, PruneInput, PruneOutput, Segment};
    use assert_matches::assert_matches;
    use reth_db::tables;
    use reth_interfaces::test_utils::{generators, generators::random_header_range};
    use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PruneSegment, B256};
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestStageDB;

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let headers = random_header_range(&mut rng, 0..100, B256::ZERO);
        db.insert_headers_with_td(headers.iter()).expect("insert headers");

        assert_eq!(db.table::<tables::CanonicalHeaders>().unwrap().len(), headers.len());
        assert_eq!(db.table::<tables::Headers>().unwrap().len(), headers.len());
        assert_eq!(db.table::<tables::HeaderTD>().unwrap().len(), headers.len());

        let test_prune = |to_block: BlockNumber, expected_result: (bool, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::Headers)
                    .unwrap(),
                to_block,
                delete_limit: 10,
            };
            let segment = Headers::new(prune_mode);

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
            let result = segment.prune(&provider, input).unwrap();
            assert_matches!(
                result,
                PruneOutput {done, pruned, checkpoint: Some(_)}
                    if (done, pruned) == expected_result
            );
            segment
                .save_checkpoint(
                    &provider,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

            let last_pruned_block_number = to_block
                .min(next_block_number_to_prune + input.delete_limit as BlockNumber / 3 - 1);

            assert_eq!(
                db.table::<tables::CanonicalHeaders>().unwrap().len(),
                headers.len() - (last_pruned_block_number + 1) as usize
            );
            assert_eq!(
                db.table::<tables::Headers>().unwrap().len(),
                headers.len() - (last_pruned_block_number + 1) as usize
            );
            assert_eq!(
                db.table::<tables::HeaderTD>().unwrap().len(),
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

        test_prune(3, (false, 9));
        test_prune(3, (true, 3));
    }

    #[test]
    fn prune_cannot_be_done() {
        let db = TestStageDB::default();

        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 1,
            // Less than total number of tables for `Headers` segment
            delete_limit: 2,
        };
        let segment = Headers::new(PruneMode::Full);

        let provider = db.factory.provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        assert_eq!(result, PruneOutput::not_done());
    }
}
