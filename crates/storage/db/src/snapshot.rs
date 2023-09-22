//! reth's snapshot creation from database tables

use crate::{
    abstraction::cursor::DbCursorRO,
    table::{Key, Table},
    transaction::DbTx,
    DatabaseError, RawKey, RawTable,
};
use reth_nippy_jar::NippyJar;
use std::ops::{RangeBounds, RangeInclusive};

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
                /// * `compression_set`: Sets of column data for compression dictionaries. Max size is 2GB. Row count is independent.
                /// * `use_hash`: Flag to indicate whether to create snapshot filters/phf from `TxHash` or `BlockHash`
                /// * `row_count`: Total rows to add to `NippyJar`. Must match row count in `range`.
                /// * `nippy_jar`: Snapshot object responsible for file generation.
                #[allow(non_snake_case)]
                pub fn [<create_snapshot$(_ $tbl)+>]<'tx,
                    $($tbl: Table<Key=K>,)+
                    HashTbl: Table<Key=K>,
                    K
                >
                (
                    tx: &impl DbTx<'tx>,
                    range: RangeInclusive<K>,
                    compression_set: Option<Vec<impl Iterator<Item = Vec<u8>>>>,
                    use_hash: bool,
                    row_count: usize,
                    nippy_jar: &mut NippyJar
                ) -> Result<(), DatabaseError>
                    where K: Key + Copy
                {
                    let range: RangeInclusive<RawKey<K>> = RawKey::new(*range.start())..=RawKey::new(*range.end());

                    handle_compression_and_indexing::<HashTbl>(
                        tx, compression_set, use_hash.then_some(range.clone()), nippy_jar
                    )?;

                    // Creates the cursors for the columns
                    $(
                        let mut [< $tbl _cursor>] = tx.cursor_read::<RawTable<$tbl>>()?;
                        let [< $tbl _iter>] = [< $tbl _cursor>]
                            .walk_range(range.clone())?
                            .into_iter()
                            .map(|row| row.map(|(_key, val)| val.take()).unwrap());

                    )+

                    // Create the snapshot from the data
                    let col_iterators: Vec<Box<dyn Iterator<Item = Vec<u8>>>> = vec![
                        $(Box::new([< $tbl _iter>]),)+
                    ];

                    nippy_jar.freeze(col_iterators, row_count as u64).unwrap();

                    Ok(())
                }
            }
        )+
    };
}

fn handle_compression_and_indexing<'tx, HashTbl: Table>(
    tx: &impl DbTx<'tx>,
    compression_set: Option<Vec<impl Iterator<Item = Vec<u8>>>>,
    hash_range: Option<impl RangeBounds<RawKey<<HashTbl as Table>::Key>>>,
    nippy_jar: &mut NippyJar,
) -> Result<(), DatabaseError> {
    // Create PHF and Filter if required
    if let Some(hash_range) = hash_range {
        let mut cursor = tx.cursor_read::<RawTable<HashTbl>>()?;

        // This might get big but ph needs the values in memory.
        let it: Vec<_> = cursor
            .walk_range(hash_range)?
            .map(|row| row.map(|(_key, value)| value.take()).unwrap())
            .collect();

        nippy_jar.prepare_index(&it).unwrap();
    }

    // Create compression dictionaries if required
    if let Some(data_sets) = compression_set {
        nippy_jar.prepare_compression(data_sets).unwrap();
    }

    Ok(())
}

generate_snapshot_func!((T1), (T1, T2), (T1, T2, T3), (T1, T2, T3, T4),);
