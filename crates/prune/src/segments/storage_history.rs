use crate::{
    segments::{
        history::prune_history_indices, PruneInput, PruneOutput, PruneOutputCheckpoint, Segment,
    },
    PrunerError,
};
use reth_db::{
    database::Database,
    models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress},
    tables,
};
use reth_primitives::{PruneMode, PruneSegment};
use reth_provider::DatabaseProviderRW;
use tracing::{instrument, trace};

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
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No storage history to prune");
                return Ok(PruneOutput::done())
            }
        };
        let range_end = *range.end();

        let mut last_changeset_pruned_block = None;
        let (pruned_changesets, done) = provider
            .prune_table_with_range::<tables::StorageChangeSet>(
                BlockNumberAddress::range(range),
                input.delete_limit / 2,
                |_| false,
                |row| last_changeset_pruned_block = Some(row.0.block_number()),
            )?;
        trace!(target: "pruner", deleted = %pruned_changesets, %done, "Pruned storage history (changesets)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more storage storage changesets to prune, set the checkpoint block number
            // to previous, so we could finish pruning its storage changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let (processed, pruned_indices) = prune_history_indices::<DB, tables::StorageHistory, _>(
            provider,
            last_changeset_pruned_block,
            |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
            |key| StorageShardedKey::last(key.address, key.sharded_key.key),
        )?;
        trace!(target: "pruner", %processed, deleted = %pruned_indices, %done, "Pruned storage history (history)" );

        Ok(PruneOutput {
            done,
            pruned: pruned_changesets + pruned_indices,
            checkpoint: Some(PruneOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, PruneOutput, Segment, StorageHistory};
    use assert_matches::assert_matches;
    use reth_db::{tables, BlockNumberList};
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_changeset_range, random_eoa_account_range},
    };
    use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PruneSegment, B256};
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestStageDB;
    use std::{collections::BTreeMap, ops::AddAssign};

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=5000, B256::ZERO, 0..1);
        db.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let accounts =
            random_eoa_account_range(&mut rng, 0..2).into_iter().collect::<BTreeMap<_, _>>();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            2..3,
            1..2,
        );
        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        let storage_occurrences = db.table::<tables::StorageHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry((key.address, key.sharded_key.key)).or_default().add_assign(1);
                map
            },
        );
        assert!(storage_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            db.table::<tables::StorageChangeSet>().unwrap().len(),
            changesets.iter().flatten().flat_map(|(_, _, entries)| entries).count()
        );

        let original_shards = db.table::<tables::StorageHistory>().unwrap();

        let test_prune = |to_block: BlockNumber, run: usize, expected_result: (bool, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::StorageHistory)
                    .unwrap(),
                to_block,
                delete_limit: 2000,
            };
            let segment = StorageHistory::new(prune_mode);

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

            let changesets = changesets
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
                    *i < input.delete_limit / 2 * run && *block_number <= to_block as usize
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
                .map(|(block_number, _, _)| if result.done {
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
                db.table::<tables::StorageChangeSet>().unwrap().len(),
                pruned_changesets.values().flatten().count()
            );

            let actual_shards = db.table::<tables::StorageHistory>().unwrap();

            let expected_shards = original_shards
                .iter()
                .filter(|(key, _)| key.sharded_key.highest_block_number > last_pruned_block_number)
                .map(|(key, blocks)| {
                    let new_blocks = blocks
                        .iter(0)
                        .skip_while(|block| *block <= last_pruned_block_number as usize)
                        .collect::<Vec<_>>();
                    (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                })
                .collect::<Vec<_>>();

            assert_eq!(actual_shards, expected_shards);

            assert_eq!(
                db.factory
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
        };

        test_prune(998, 1, (false, 1000));
        test_prune(998, 2, (true, 998));
        test_prune(1400, 3, (true, 804));
    }
}
