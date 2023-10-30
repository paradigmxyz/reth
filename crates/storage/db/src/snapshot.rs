//! reth's snapshot creation from database tables and access

use crate::{
    abstraction::cursor::DbCursorRO,
    table::{Decompress, Key, Table},
    transaction::DbTx,
    RawKey, RawTable,
};
use derive_more::{Deref, DerefMut};
use reth_interfaces::{RethError, RethResult};
use reth_nippy_jar::{ColumnResult, MmapHandle, NippyJar, NippyJarCursor, PHFKey};
use reth_primitives::{snapshot::SegmentHeader, B256};
use reth_tracing::tracing::*;
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, ops::RangeInclusive};

/// Macro that generates snapshot creation functions that take an arbitratry number of [`Table`] and
/// creates a [`NippyJar`] file out of their [`Table::Value`]. Each list of [`Table::Value`] from a
/// table is a column of values.
///
/// Has membership filter set and compression dictionary support.
macro_rules! generate_snapshot_func {
    ($(($($tbl:ident),+)),+ $(,)? ) => {
        $(
            paste::item! {
                /// Creates a snapshot from specified tables. Each table's `Value` iterator represents a column.
                ///
                /// **Ensure the range contains the same number of rows.**
                ///
                /// * `tx`: Database transaction.
                /// * `range`: Data range for columns in tables.
                /// * `additional`: Additional columns which can't be straight straightforwardly walked on.
                /// * `keys`: Iterator of keys (eg. `TxHash` or `BlockHash`) with length equal to `row_count` and ordered by future column insertion from `range`.
                /// * `dict_compression_set`: Sets of column data for compression dictionaries. Max size is 2GB. Row count is independent.
                /// * `row_count`: Total rows to add to `NippyJar`. Must match row count in `range`.
                /// * `nippy_jar`: Snapshot object responsible for file generation.
                #[allow(non_snake_case)]
                pub fn [<create_snapshot$(_ $tbl)+>]<
                    $($tbl: Table<Key=K>,)+
                    K,
                    H: for<'a> Deserialize<'a> + Send + Serialize + Sync + std::fmt::Debug
                >
                (
                    tx: &impl DbTx,
                    range: RangeInclusive<K>,
                    additional: Option<Vec<Box<dyn Iterator<Item = Result<Vec<u8>, Box<dyn StdError + Send + Sync>>>>>>,
                    dict_compression_set: Option<Vec<impl Iterator<Item = Vec<u8>>>>,
                    keys: Option<impl Iterator<Item = ColumnResult<impl PHFKey>>>,
                    row_count: usize,
                    nippy_jar: &mut NippyJar<H>
                ) -> RethResult<()>
                    where K: Key + Copy
                {
                    let additional = additional.unwrap_or_default();
                    debug!(target: "reth::snapshot", ?range, "Creating snapshot {:?} and {} more columns.", vec![$($tbl::NAME,)+], additional.len());

                    let range: RangeInclusive<RawKey<K>> = RawKey::new(*range.start())..=RawKey::new(*range.end());

                    // Create PHF and Filter if required
                    if let Some(keys) = keys {
                        debug!(target: "reth::snapshot", "Calculating Filter, PHF and offset index list");
                        nippy_jar.prepare_index(keys, row_count)?;
                        debug!(target: "reth::snapshot", "Filter, PHF and offset index list calculated.");
                    }

                    // Create compression dictionaries if required
                    if let Some(data_sets) = dict_compression_set {
                        debug!(target: "reth::snapshot", "Creating compression dictionaries.");
                        nippy_jar.prepare_compression(data_sets)?;
                        debug!(target: "reth::snapshot", "Compression dictionaries created.");
                    }

                    // Creates the cursors for the columns
                    $(
                        let mut [< $tbl _cursor>] = tx.cursor_read::<RawTable<$tbl>>()?;
                        let [< $tbl _iter>] = [< $tbl _cursor>]
                            .walk_range(range.clone())?
                            .into_iter()
                            .map(|row|
                                row
                                    .map(|(_key, val)| val.into_value())
                                    .map_err(|e| Box::new(e) as Box<dyn StdError + Send + Sync>)
                            );

                    )+

                    // Create the snapshot from the data
                    let col_iterators: Vec<Box<dyn Iterator<Item = Result<Vec<u8>,_>>>> = vec![
                        $(Box::new([< $tbl _iter>]),)+
                    ];


                    debug!(target: "reth::snapshot", jar=?nippy_jar, "Generating snapshot file.");

                    nippy_jar.freeze(col_iterators.into_iter().chain(additional).collect(), row_count as u64)?;

                    debug!(target: "reth::snapshot", jar=?nippy_jar, "Snapshot file generated.");

                    Ok(())
                }
            }
        )+
    };
}

generate_snapshot_func!((T1), (T1, T2), (T1, T2, T3), (T1, T2, T3, T4), (T1, T2, T3, T4, T5),);

/// Cursor of a snapshot segment.
#[derive(Debug, Deref, DerefMut)]
pub struct SnapshotCursor<'a>(NippyJarCursor<'a, SegmentHeader>);

impl<'a> SnapshotCursor<'a> {
    /// Returns a new [`SnapshotCursor`].
    pub fn new(
        jar: &'a NippyJar<SegmentHeader>,
        mmap_handle: MmapHandle,
    ) -> Result<Self, RethError> {
        Ok(Self(NippyJarCursor::with_handle(jar, mmap_handle)?))
    }

    /// Gets a row of values.
    pub fn get<const SELECTOR: usize, const COLUMNS: usize>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<Vec<&'_ [u8]>>> {
        let row = match key_or_num {
            KeyOrNumber::Hash(k) => self.row_by_key_with_cols::<SELECTOR, COLUMNS>(k),
            KeyOrNumber::Number(n) => {
                let offset = self.jar().user_header().start();
                if offset > n {
                    return Ok(None)
                }
                self.row_by_number_with_cols::<SELECTOR, COLUMNS>((n - offset) as usize)
            }
        }?;

        Ok(row)
    }

    /// Gets one column value from a row.
    pub fn get_one<T: Decompress, const SELECTOR: usize, const COLUMNS: usize>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<T>> {
        let row = self.get::<SELECTOR, COLUMNS>(key_or_num)?;

        match row {
            Some(row) => Ok(Some(T::decompress(row[0])?)),
            None => Ok(None),
        }
    }

    /// Gets two column values from a row.
    pub fn get_two<T: Decompress, K: Decompress, const SELECTOR: usize, const COLUMNS: usize>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<(T, K)>> {
        let row = self.get::<SELECTOR, COLUMNS>(key_or_num)?;

        match row {
            Some(row) => Ok(Some((T::decompress(row[0])?, K::decompress(row[1])?))),
            None => Ok(None),
        }
    }

    /// Gets three column values from a row.
    pub fn get_three<
        T: Decompress,
        K: Decompress,
        J: Decompress,
        const SELECTOR: usize,
        const COLUMNS: usize,
    >(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<(T, K, J)>> {
        let row = self.get::<SELECTOR, COLUMNS>(key_or_num)?;

        match row {
            Some(row) => {
                Ok(Some((T::decompress(row[0])?, K::decompress(row[1])?, J::decompress(row[2])?)))
            }
            None => Ok(None),
        }
    }
}

/// Either a key _or_ a block number
#[derive(Debug)]
pub enum KeyOrNumber<'a> {
    /// A slice used as a key. Usually a block hash
    Hash(&'a [u8]),
    /// A block number
    Number(u64),
}

impl<'a> From<&'a B256> for KeyOrNumber<'a> {
    fn from(value: &'a B256) -> Self {
        KeyOrNumber::Hash(value.as_slice())
    }
}

impl<'a> From<u64> for KeyOrNumber<'a> {
    fn from(value: u64) -> Self {
        KeyOrNumber::Number(value)
    }
}

/// Snapshot segment total columns.
pub const HEADER_COLUMNS: usize = 3;
/// Selector for header.
pub const S_HEADER: usize = 0b001;
/// Selector for header td.
pub const S_HEADER_TD: usize = 0b010;
/// Selector for header hash.
pub const S_HEADER_HASH: usize = 0b100;
/// Selector for header td and header hash.
pub const S_HEADER_TD_WITH_HASH: usize = 0b110;
/// Selector for header and header hash.
pub const S_HEADER_WITH_HASH: usize = 0b101;
