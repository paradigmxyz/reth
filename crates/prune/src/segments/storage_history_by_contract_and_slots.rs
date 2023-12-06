use crate::{
    segments::{history::prune_history_indices, PruneInput, PruneOutput, Segment},
    PrunerError,
};
use reth_db::{
    database::Database,
    models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress},
    tables,
};
use reth_primitives::{
    PruneCheckpoint, PruneMode, PruneSegment, StorageHistoryPruneConfig, MINIMUM_PRUNING_DISTANCE,
};
use reth_provider::{DatabaseProviderRW, PruneCheckpointWriter};
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct StorageHistoryByContractAndSlots {
    config: StorageHistoryPruneConfig,
}

impl StorageHistoryByContractAndSlots {
    pub fn new(config: StorageHistoryPruneConfig) -> Self {
        Self { config }
    }
}

impl<DB: Database> Segment<DB> for StorageHistoryByContractAndSlots {
    fn segment(&self) -> PruneSegment {
        PruneSegment::StorageHistoryFilteredByContractAndSlots
    }

    fn mode(&self) -> Option<PruneMode> {
        None
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError> {
        // Storage history filtering removes every storage changes possible except the ones in the
        // list. So, for the other storage changes it's as if they had a `PruneMode::Distance()` of
        // `MINIMUM_PRUNING_DISTANCE`.
        let to_block = PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)
            .prune_target_block(
                input.to_block,
                PruneSegment::StorageHistoryFilteredByContractAndSlots,
            )?
            .map(|(bn, _)| bn)
            .unwrap_or_default();

        // Get status checkpoint from latest run
        let mut last_pruned_block =
            input.previous_checkpoint.and_then(|checkpoint| checkpoint.block_number);

        let initial_last_pruned_block = last_pruned_block;

        // Figure out what storage history have already been pruned, so we can have an accurate
        // `address_slots_filter`
        let address_slots_filter = self.config.group_by_block(input.to_block, last_pruned_block)?;

        // Splits all addresses in different block ranges. Each block range will have its own
        // filter address and slot list and will check it while going through the table
        //
        // Example:
        // For an `address_filter` such as:
        // { block9: [(a1, []), (a2, [])], block20: [(a3, [s1]), (a4, []), (a5, [s1, s2])] }
        //
        // The following structures will be created in the exact order as showed:
        // `block_ranges`: [
        //    (block0, block8, 0 addresses),
        //    (block9, block19, 2 addresses),
        //    (block20, to_block, 5 addresses)
        //  ]
        // `filtered_address_slots`: [(a1, []), (a2, []), (a3, [s1]), (a4, []), (a5, [s1, s2])]
        //
        // The first range will delete all storage history between block0 - block8
        // The second range will delete all storage history between block9 - 19, except the ones
        // with these addresses and slots: [a1: all slots, a2: all slot]. The third range will
        // delete all storage history between block20 - to_block, except the ones with these
        // addresses and slots: [a1: all slots, a2: all slots, a3: slot1, a4: all slots, a5: slot1
        // and slot2]
        let mut block_ranges = vec![];
        let mut blocks_iter = address_slots_filter.iter().peekable();
        let mut filtered_address_slots = vec![];

        while let Some((start_block, addresses_and_slots)) = blocks_iter.next() {
            filtered_address_slots.extend_from_slice(addresses_and_slots);

            // This will clear all storage history before the first appearance of a contract address
            // or since the block after the last pruned one.
            if block_ranges.is_empty() {
                let init = last_pruned_block.map(|b| b + 1).unwrap_or_default();
                if init < *start_block {
                    block_ranges.push((init, *start_block - 1, 0));
                }
            }

            let end_block =
                blocks_iter.peek().map(|(next_block, _)| *next_block - 1).unwrap_or(to_block);

            // Addresses in lower block ranges, are still included in the inclusion list for future
            // ranges.
            block_ranges.push((*start_block, end_block, filtered_address_slots.len()));
        }

        trace!(
            target: "pruner",
            ?block_ranges,
            ?filtered_address_slots,
            "Calculated block ranges and filtered addresses and slots",
        );

        let mut limit = input.delete_limit;
        let mut done = true;
        let mut last_changeset_pruned_block = None;
        for (start_block, end_block, num_addresses) in block_ranges {
            let block_range = start_block..=end_block;

            // Delete storage history (changesets), except the ones in the inclusion list
            let mut last_changeset_skipped_block = 0;
            let rows;
            (rows, done) = provider.prune_table_with_range::<tables::StorageChangeSet>(
                BlockNumberAddress::range(block_range.clone()),
                limit,
                |(block_num_address, storage_entry)| {
                    let mut skip = num_addresses > 0;
                    if let Some(&address_slots) = filtered_address_slots[..num_addresses]
                        .iter()
                        .find(|&address_slot| address_slot.address == block_num_address.address())
                    {
                        let slots = &address_slots.slots;
                        if slots.is_empty() {
                            // if slots is empty, save all slots
                            skip &= true;
                        } else {
                            // else, save only slots specified in filtered_address_slots
                            skip &= slots.contains(&storage_entry.key);
                        }
                    } else {
                        // if address is not in the list, delete its storage history
                        skip &= false;
                    }
                    if skip {
                        last_changeset_skipped_block = block_num_address.block_number();
                    }
                    skip
                },
                |row| last_changeset_pruned_block = Some(row.0.block_number()),
            )?;
            trace!(target: "pruner", %rows, %done, ?block_range, "Pruned storage history (changesets)");

            limit = limit.saturating_sub(rows);

            let last_changeset_pruned_block = last_changeset_pruned_block
                // If there's more account storage changesets to prune, set the checkpoint block
                // number to previous, so we could finish pruning its storage
                // changesets on the next run.
                .map(
                    |block_number| if done { block_number } else { block_number.saturating_sub(1) },
                )
                .unwrap_or(to_block);

            last_pruned_block = Some(last_changeset_pruned_block);

            let (processed, deleted) = prune_history_indices::<DB, tables::StorageHistory, _>(
                provider,
                last_changeset_pruned_block,
                |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
                |key| StorageShardedKey::last(key.address, key.sharded_key.key),
                |storage_sharded_key| {
                    if let Some(&address_slots) = filtered_address_slots[..num_addresses]
                        .iter()
                        .find(|&address_slots| address_slots.address == storage_sharded_key.address)
                    {
                        let slots = &address_slots.slots;
                        if slots.is_empty() {
                            // if slots is empty, save all slots
                            true
                        } else {
                            // else, save only slots specified in filtered_address_slots
                            slots.contains(&storage_sharded_key.sharded_key.key)
                        }
                    } else {
                        // if address is not in the list, delete its storage history indices
                        false
                    }
                },
            )?;
            trace!(target: "pruner", %processed, %deleted, %done, "Pruned storage history (history)" );

            if limit == 0 {
                done &= end_block == to_block;
                break
            }
        }

        // If there are contracts using `PruneMode::Distance(_)` there will be storage history
        // before `to_block` that become eligible to be pruned in future runs. Therefore,
        // our checkpoint is not actually `to_block`, but the `lowest_block_with_distance`
        // from any contract. This ensures that in future pruner runs we can prune all these
        // storage history between the previous `lowest_block_with_distance` and the new one using
        // `get_next_block_range_from_checkpoint`.
        //
        // Only applies if we were able to prune everything intended for this run, otherwise the
        // checkpoing is the `last_pruned_block`.
        let prune_mode_block = self
            .config
            .lowest_block_with_distance(input.to_block, initial_last_pruned_block)?
            .unwrap_or(to_block);

        provider.save_prune_checkpoint(
            PruneSegment::StorageHistoryFilteredByContractAndSlots,
            PruneCheckpoint {
                block_number: Some(prune_mode_block.min(last_pruned_block.unwrap_or(u64::MAX))),
                tx_number: None,
                prune_mode: PruneMode::Before(prune_mode_block),
            },
        )?;

        Ok(PruneOutput { done, pruned: input.delete_limit - limit, checkpoint: None })
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, Segment, StorageHistoryByContractAndSlots};
    use assert_matches::assert_matches;
    use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_changeset_range, random_eoa_account_range},
    };
    use reth_primitives::{
        AddressAndSlots, PruneMode, PruneSegment, StorageHistoryPruneConfig, B256,
        MINIMUM_PRUNING_DISTANCE,
    };
    use reth_provider::PruneCheckpointReader;
    use reth_stages::test_utils::TestStageDB;
    use std::{collections::BTreeMap, ops::AddAssign};

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let tip = 20000;
        let blocks = random_block_range(&mut rng, 0..=tip, B256::ZERO, 0..1);
        db.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let accounts =
            random_eoa_account_range(&mut rng, 1..3).into_iter().collect::<BTreeMap<_, _>>();

        let address1 = *accounts.iter().next().map(|(addr, _)| addr).unwrap();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            1..2,
            1..2,
        );
        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        let storage_occurences = db.table::<tables::StorageHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry((key.address, key.sharded_key.key)).or_default().add_assign(1);
                map
            },
        );
        assert!(storage_occurences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            db.table::<tables::StorageChangeSet>().unwrap().len(),
            changesets.iter().flatten().flat_map(|(_, _, entries)| entries).count()
        );

        let run_prune = || {
            let provider = db.factory.provider_rw().unwrap();

            let prune_before_block: usize = 2300;
            let prune_mode = PruneMode::Before(prune_before_block as u64);

            let result =
                StorageHistoryByContractAndSlots::new(StorageHistoryPruneConfig(BTreeMap::from([
                    (AddressAndSlots { address: address1, slots: vec![] }, prune_mode),
                ])))
                .prune(
                    &provider,
                    PruneInput {
                        previous_checkpoint: db
                            .factory
                            .provider()
                            .unwrap()
                            .get_prune_checkpoint(
                                PruneSegment::StorageHistoryFilteredByContractAndSlots,
                            )
                            .unwrap(),
                        to_block: tip,
                        delete_limit: 2000,
                    },
                );
            provider.commit().expect("commit");

            assert_matches!(result, Ok(_));
            let output = result.unwrap();

            output.done
        };

        while !run_prune() {}

        let provider = db.factory.provider().unwrap();
        let mut cursor = provider.tx_ref().cursor_read::<tables::StorageChangeSet>().unwrap();
        let walker = cursor.walk(None).unwrap();

        for storage_change_set in walker {
            let (block_num_address, _storage_entry) = storage_change_set.unwrap();

            // Either we only find our contract and slots, or the changes are part of the unprunable
            // changes set by `tip - MINIMUM_PRUNING_DISTANCE`
            assert!(
                block_num_address.address() == address1 ||
                    block_num_address.block_number() > tip - MINIMUM_PRUNING_DISTANCE,
            );
        }
    }
}
