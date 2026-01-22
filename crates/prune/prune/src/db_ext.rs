use crate::PruneLimiter;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, RangeWalker},
    table::{DupSort, Table, TableRow},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use std::{fmt::Debug, ops::RangeBounds};
use tracing::debug;

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

            let done = self.prune_table_with_range_step(
                &mut walker,
                limiter,
                &mut skip_filter,
                &mut delete_callback,
            )?;

            if done {
                break true
            }
            deleted_entries += 1;
        };

        Ok((deleted_entries, done))
    }

    /// Steps once with the given walker and prunes the entry in the table.
    ///
    /// Returns `true` if the walker is finished, `false` if it may have more data to prune.
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
    ) -> Result<bool, DatabaseError> {
        let Some(res) = walker.next() else { return Ok(true) };

        let row = res?;

        if !skip_filter(&row) {
            walker.delete_current()?;
            limiter.increment_deleted_entries_count();
            delete_callback(row);
        }

        Ok(false)
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
}

impl<Tx> DbTxPruneExt for Tx where Tx: DbTxMut + DbTx {}

#[cfg(test)]
mod tests {
    use super::DbTxPruneExt;
    use crate::PruneLimiter;
    use reth_db_api::tables;
    use reth_primitives_traits::SignerRecoverable;
    use reth_provider::{DBProvider, DatabaseProviderFactory};
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
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
}
