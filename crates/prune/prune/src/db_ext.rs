use crate::PruneLimiter;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, RangeWalker},
    table::{DupSort, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use std::{fmt::Debug, ops::RangeBounds};
use tracing::debug;

/// Result of a single prune step in [`DbTxPruneExt::prune_table_with_range_step`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct PruneStepResult {
    /// `true` if the walker is finished, `false` if it may have more data to prune.
    done: bool,
    /// `true` if the current entry was deleted, `false` if it was skipped.
    deleted: bool,
}

pub(crate) trait DbTxPruneExt: DbTxMut + DbTx {
    /// Clear the entire table in a single operation.
    ///
    /// This is much faster than iterating entry-by-entry for `PruneMode::Full`.
    /// Returns the number of entries that were in the table.
    fn clear_table<T: Table>(&self) -> Result<usize, DatabaseError> {
        let count = self.entries::<T>()?;
        <Self as DbTxMut>::clear::<T>(self)?;
        Ok(count)
    }

    /// Prune the table for the specified pre-sorted key iterator.
    ///
    /// Returns number of rows pruned.
    fn prune_table_with_iterator<T: Table>(
        &self,
        keys: impl IntoIterator<Item = T::Key>,
        limiter: &mut PruneLimiter,
        mut delete_callback: impl FnMut(TableRow<T>),
    ) -> Result<(usize, bool), DatabaseError> {
        let mut cursor = self.cursor_write::<T>()?;
        let mut keys = keys.into_iter().peekable();

        let mut deleted_entries = 0;

        let mut done = true;
        while keys.peek().is_some() {
            if limiter.is_limit_reached() {
                debug!(
                    target: "providers::db",
                    ?limiter,
                    deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                    time_limit = %limiter.is_time_limit_reached(),
                    table = %T::NAME,
                    "Pruning limit reached"
                );
                done = false;
                break
            }

            let key = keys.next().expect("peek() said Some");
            let row = cursor.seek_exact(key)?;
            if let Some(row) = row {
                cursor.delete_current()?;
                limiter.increment_deleted_entries_count();
                deleted_entries += 1;
                delete_callback(row);
            }
        }

        Ok((deleted_entries, done))
    }

    /// Prune the table for the specified key range.
    ///
    /// Returns number of rows pruned.
    fn prune_table_with_range<T: Table>(
        &self,
        keys: impl RangeBounds<T::Key> + Clone + Debug,
        limiter: &mut PruneLimiter,
        mut skip_filter: impl FnMut(&TableRow<T>) -> bool,
        mut delete_callback: impl FnMut(TableRow<T>),
    ) -> Result<(usize, bool), DatabaseError> {
        let mut cursor = self.cursor_write::<T>()?;
        let mut walker = cursor.walk_range(keys)?;

        let mut deleted_entries = 0;

        let done = loop {
            // check for time out must be done in this scope since it's not done in
            // `prune_table_with_range_step`
            if limiter.is_limit_reached() {
                debug!(
                    target: "providers::db",
                    ?limiter,
                    deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                    time_limit = %limiter.is_time_limit_reached(),
                    table = %T::NAME,
                    "Pruning limit reached"
                );
                break false
            }

            let result = self.prune_table_with_range_step(
                &mut walker,
                limiter,
                &mut skip_filter,
                &mut delete_callback,
            )?;

            if result.deleted {
                deleted_entries += 1;
            }

            if result.done {
                break true
            }
        };

        Ok((deleted_entries, done))
    }

    /// Steps once with the given walker and prunes the entry in the table.
    ///
    /// CAUTION: Pruner limits are not checked. This allows for a clean exit of a prune run that's
    /// pruning different tables concurrently, by letting them step to the same height before
    /// timing out.
    fn prune_table_with_range_step<T: Table>(
        &self,
        walker: &mut RangeWalker<'_, T, Self::CursorMut<T>>,
        limiter: &mut PruneLimiter,
        skip_filter: &mut impl FnMut(&TableRow<T>) -> bool,
        delete_callback: &mut impl FnMut(TableRow<T>),
    ) -> Result<PruneStepResult, DatabaseError> {
        let Some(res) = walker.next() else {
            return Ok(PruneStepResult { done: true, deleted: false })
        };

        let row = res?;

        if skip_filter(&row) {
            Ok(PruneStepResult { done: false, deleted: false })
        } else {
            walker.delete_current()?;
            limiter.increment_deleted_entries_count();
            delete_callback(row);
            Ok(PruneStepResult { done: false, deleted: true })
        }
    }

    /// Prune a DUPSORT table for the specified key range.
    ///
    /// Returns number of rows pruned.
    #[expect(unused)]
    fn prune_dupsort_table_with_range<T: DupSort>(
        &self,
        keys: impl RangeBounds<T::Key> + Clone + Debug,
        limiter: &mut PruneLimiter,
        mut delete_callback: impl FnMut(TableRow<T>),
    ) -> Result<(usize, bool), DatabaseError> {
        let starting_entries = self.entries::<T>()?;
        let mut cursor = self.cursor_dup_write::<T>()?;
        let mut walker = cursor.walk_range(keys)?;

        let done = loop {
            if limiter.is_limit_reached() {
                debug!(
                    target: "providers::db",
                    ?limiter,
                    deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                    time_limit = %limiter.is_time_limit_reached(),
                    table = %T::NAME,
                    "Pruning limit reached"
                );
                break false
            }

            let Some(res) = walker.next() else { break true };
            let row = res?;

            walker.delete_current_duplicates()?;
            limiter.increment_deleted_entries_count();
            delete_callback(row);
        };

        debug!(
            target: "providers::db",
            table=?T::NAME,
            cursor_current=?cursor.current(),
            "done walking",
        );

        let ending_entries = self.entries::<T>()?;

        Ok((starting_entries - ending_entries, done))
    }

    /// Prune duplicate entries for a single DUPSORT key.
    ///
    /// Returns the number of rows pruned and whether all duplicate entries for the key were
    /// deleted.
    #[allow(dead_code)]
    fn prune_dupsort_key_entries<T: DupSort>(
        &self,
        key: T::Key,
        limiter: &mut PruneLimiter,
    ) -> Result<(usize, bool), DatabaseError> {
        let mut cursor = self.cursor_dup_write::<T>()?;
        let mut entry = cursor.seek_exact(key)?;

        let mut deleted_entries = 0;

        while entry.is_some() && !limiter.is_limit_reached() {
            cursor.delete_current()?;
            limiter.increment_deleted_entries_count();
            deleted_entries += 1;
            entry = cursor.next_dup()?;
        }

        // an entry remaining means the loop stopped because a limit was reached
        let done = entry.is_none();
        if !done {
            debug!(
                target: "providers::db",
                ?limiter,
                deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                time_limit = %limiter.is_time_limit_reached(),
                table = %T::NAME,
                "Pruning limit reached"
            );
        }

        Ok((deleted_entries, done))
    }
}

impl<Tx> DbTxPruneExt for Tx where Tx: DbTxMut + DbTx {}

#[cfg(test)]
mod tests {
    use super::DbTxPruneExt;
    use crate::PruneLimiter;
    use alloy_primitives::{B256, U256};
    use reth_db_api::{tables, transaction::DbTxMut};
    use reth_primitives_traits::{SignerRecoverable, StorageEntry};
    use reth_provider::{DBProvider, DatabaseProviderFactory};
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    struct CountingIter {
        data: Vec<u64>,
        calls: Arc<AtomicUsize>,
    }

    impl CountingIter {
        fn new(data: Vec<u64>, calls: Arc<AtomicUsize>) -> Self {
            Self { data, calls }
        }
    }

    struct CountingIntoIter {
        inner: std::vec::IntoIter<u64>,
        calls: Arc<AtomicUsize>,
    }

    impl Iterator for CountingIntoIter {
        type Item = u64;
        fn next(&mut self) -> Option<Self::Item> {
            let res = self.inner.next();
            self.calls.fetch_add(1, Ordering::SeqCst);
            res
        }
    }

    impl IntoIterator for CountingIter {
        type Item = u64;
        type IntoIter = CountingIntoIter;
        fn into_iter(self) -> Self::IntoIter {
            CountingIntoIter { inner: self.data.into_iter(), calls: self.calls }
        }
    }

    fn storage_entry(slot_byte: u8) -> StorageEntry {
        StorageEntry { key: B256::with_last_byte(slot_byte), value: U256::from(slot_byte) }
    }

    fn insert_hashed_storages(db: &TestStageDB, rows: impl IntoIterator<Item = (u8, u8)>) {
        let provider = db.factory.database_provider_rw().unwrap();
        for (address_byte, slot_byte) in rows {
            provider
                .tx_ref()
                .put::<tables::HashedStorages>(
                    B256::with_last_byte(address_byte),
                    storage_entry(slot_byte),
                )
                .expect("insert hashed storage");
        }
        provider.commit().expect("commit");
    }

    fn hashed_storage_slots(db: &TestStageDB, address_byte: u8) -> Vec<B256> {
        db.table::<tables::HashedStorages>()
            .unwrap()
            .into_iter()
            .filter_map(|(key, entry)| {
                (key == B256::with_last_byte(address_byte)).then_some(entry.key)
            })
            .collect()
    }

    fn prune_hashed_storage_key(
        db: &TestStageDB,
        address_byte: u8,
        limiter: &mut PruneLimiter,
    ) -> (usize, bool) {
        let provider = db.factory.database_provider_rw().unwrap();
        let result = provider
            .tx_ref()
            .prune_dupsort_key_entries::<tables::HashedStorages>(
                B256::with_last_byte(address_byte),
                limiter,
            )
            .expect("prune hashed storages");
        provider.commit().expect("commit");
        result
    }

    #[test]
    fn prune_table_with_iterator_early_exit_does_not_overconsume() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=3,
            BlockRangeParams {
                parent: Some(alloy_primitives::B256::ZERO),
                tx_count: 2..3,
                ..Default::default()
            },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let mut tx_senders = Vec::new();
        for block in &blocks {
            tx_senders.reserve_exact(block.transaction_count());
            for transaction in &block.body().transactions {
                tx_senders.push((
                    tx_senders.len() as u64,
                    transaction.recover_signer().expect("recover signer"),
                ));
            }
        }
        let total = tx_senders.len();
        db.insert_transaction_senders(tx_senders).expect("insert transaction senders");

        let provider = db.factory.database_provider_rw().unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let keys: Vec<u64> = (0..total as u64).collect();
        let counting_iter = CountingIter::new(keys, calls.clone());

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(2);

        let (pruned, done) = provider
            .tx_ref()
            .prune_table_with_iterator::<tables::TransactionSenders>(
                counting_iter,
                &mut limiter,
                |_| {},
            )
            .expect("prune");

        assert_eq!(pruned, 2);
        assert!(!done);
        assert_eq!(calls.load(Ordering::SeqCst), pruned + 1);

        provider.commit().expect("commit");
        assert_eq!(db.table::<tables::TransactionSenders>().unwrap().len(), total - 2);
    }

    #[test]
    fn prune_table_with_iterator_consumes_to_end_reports_done() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=2,
            BlockRangeParams {
                parent: Some(alloy_primitives::B256::ZERO),
                tx_count: 1..2,
                ..Default::default()
            },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let mut tx_senders = Vec::new();
        for block in &blocks {
            for transaction in &block.body().transactions {
                tx_senders.push((
                    tx_senders.len() as u64,
                    transaction.recover_signer().expect("recover signer"),
                ));
            }
        }
        let total = tx_senders.len();
        db.insert_transaction_senders(tx_senders).expect("insert transaction senders");

        let provider = db.factory.database_provider_rw().unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let keys: Vec<u64> = (0..total as u64).collect();
        let counting_iter = CountingIter::new(keys, calls.clone());

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(usize::MAX);

        let (pruned, done) = provider
            .tx_ref()
            .prune_table_with_iterator::<tables::TransactionSenders>(
                counting_iter,
                &mut limiter,
                |_| {},
            )
            .expect("prune");

        assert_eq!(pruned, total);
        assert!(done);
        assert_eq!(calls.load(Ordering::SeqCst), total + 1);

        provider.commit().expect("commit");
        assert_eq!(db.table::<tables::TransactionSenders>().unwrap().len(), 0);
    }

    #[test]
    fn prune_dupsort_key_entries_resumes_with_deleted_entries_budget() {
        let db = TestStageDB::default();
        insert_hashed_storages(&db, (0..5).map(|slot| (1, slot)));
        insert_hashed_storages(&db, (10..12).map(|slot| (2, slot)));

        let mut total_deleted = 0;

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(2);
        let (deleted, done) = prune_hashed_storage_key(&db, 1, &mut limiter);
        total_deleted += deleted;
        assert_eq!((deleted, done), (2, false));
        assert_eq!(
            hashed_storage_slots(&db, 1),
            vec![B256::with_last_byte(2), B256::with_last_byte(3), B256::with_last_byte(4),]
        );
        assert_eq!(
            hashed_storage_slots(&db, 2),
            vec![B256::with_last_byte(10), B256::with_last_byte(11)]
        );

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(2);
        let (deleted, done) = prune_hashed_storage_key(&db, 1, &mut limiter);
        total_deleted += deleted;
        assert_eq!((deleted, done), (2, false));
        assert_eq!(hashed_storage_slots(&db, 1), vec![B256::with_last_byte(4)]);
        assert_eq!(
            hashed_storage_slots(&db, 2),
            vec![B256::with_last_byte(10), B256::with_last_byte(11)]
        );

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(2);
        let (deleted, done) = prune_hashed_storage_key(&db, 1, &mut limiter);
        total_deleted += deleted;
        assert_eq!((deleted, done), (1, true));
        assert!(hashed_storage_slots(&db, 1).is_empty());
        assert_eq!(
            hashed_storage_slots(&db, 2),
            vec![B256::with_last_byte(10), B256::with_last_byte(11)]
        );
        assert_eq!(total_deleted, 5);
    }

    #[test]
    fn prune_dupsort_key_entries_stops_on_time_limit() {
        let db = TestStageDB::default();
        insert_hashed_storages(&db, (0..3).map(|slot| (1, slot)));

        let mut limiter = PruneLimiter::default().set_time_limit(Duration::from_nanos(1));
        std::thread::sleep(Duration::from_millis(1));

        let (deleted, done) = prune_hashed_storage_key(&db, 1, &mut limiter);

        assert_eq!((deleted, done), (0, false));
        assert!(limiter.is_time_limit_reached());
        assert_eq!(
            hashed_storage_slots(&db, 1),
            vec![B256::with_last_byte(0), B256::with_last_byte(1), B256::with_last_byte(2)]
        );
    }

    #[test]
    fn prune_dupsort_key_entries_missing_key_is_done() {
        let db = TestStageDB::default();
        insert_hashed_storages(&db, (0..3).map(|slot| (1, slot)));

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(2);
        let result = prune_hashed_storage_key(&db, 2, &mut limiter);

        assert_eq!(result, (0, true));
        assert_eq!(
            hashed_storage_slots(&db, 1),
            vec![B256::with_last_byte(0), B256::with_last_byte(1), B256::with_last_byte(2)]
        );
    }

    #[test]
    fn prune_dupsort_key_entries_leaves_other_keys_untouched() {
        let db = TestStageDB::default();
        insert_hashed_storages(&db, (0..3).map(|slot| (1, slot)));
        insert_hashed_storages(&db, (10..13).map(|slot| (2, slot)));

        let mut limiter = PruneLimiter::default();
        let result = prune_hashed_storage_key(&db, 1, &mut limiter);

        assert_eq!(result, (3, true));
        assert!(hashed_storage_slots(&db, 1).is_empty());
        assert_eq!(
            hashed_storage_slots(&db, 2),
            vec![B256::with_last_byte(10), B256::with_last_byte(11), B256::with_last_byte(12),]
        );
    }
}
