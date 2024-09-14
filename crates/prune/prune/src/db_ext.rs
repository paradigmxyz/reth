use std::{fmt::Debug, ops::RangeBounds};

use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, RangeWalker},
    table::{Table, TableRow},
    transaction::DbTxMut,
    DatabaseError,
};
use reth_prune_types::PruneLimiter;
use tracing::debug;

pub(crate) trait DbTxPruneExt: DbTxMut {
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
        let mut keys = keys.into_iter();

        let mut deleted_entries = 0;

        for key in &mut keys {
            if limiter.is_limit_reached() {
                debug!(
                    target: "providers::db",
                    ?limiter,
                    deleted_entries_limit = %limiter.is_deleted_entries_limit_reached(),
                    time_limit = %limiter.is_time_limit_reached(),
                    table = %T::NAME,
                    "Pruning limit reached"
                );
                break
            }

            let row = cursor.seek_exact(key)?;
            if let Some(row) = row {
                cursor.delete_current()?;
                limiter.increment_deleted_entries_count();
                deleted_entries += 1;
                delete_callback(row);
            }
        }

        let done = keys.next().is_none();
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
}

impl<Tx> DbTxPruneExt for Tx where Tx: DbTxMut {}
