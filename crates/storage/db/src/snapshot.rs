//! reth's snapshot creation from database tables

use crate::{
    abstraction::cursor::DbCursorRO, table::Table, transaction::DbTx, DatabaseError, RawKey,
    RawTable,
};
use reth_nippy_jar::NippyJar;
use std::ops::RangeBounds;

/// Macro that generates snapshot creation functions that take an arbitratry number of [`Table`] and
/// creates a [`NippyJar`] file out of their [`Table::Value`]. Each list of [`Table::Value`] from a
/// table is a column of values.
///
/// Has membership filter set and compression dictionary support.
macro_rules! generate_snapshot_func {
    ($(($($tbl:ident),+)),+ $(,)? ) => {
        $(
            paste::item! {
                ///  Creates a snapshot from the given tables. Each iterator of a `Table::Value` corresponds to a
                /// column.
                ///
                /// **User must make sure that the number of rows is the same in all ranges.**
                ///
                /// * `tx`: database transaction.
                /// * `Tn_range`: `columnN` data range in the table.
                /// * `compression_set`: column data sets for compression dictionaries. This can only be
                /// done over a subset of the whole table (2GB maximum for now). Number of entries **is not tied to
                /// `row_count`**.
                /// * `hash_range`: range of a table which `Table::Value` is either `TxHash` or `BlockHash`,
                ///   depending on the usecase.
                /// * `row_count` total number of rows that will be added to [`NippyJar`]. **Must be the same across
                ///   all `Tn_range` and `hash_range`.**
                /// * `nippy_jar`: snapshot format object that will create all the necessary files.
                #[allow(non_snake_case)]
                pub fn [<create_snapshot$(_ $tbl)+>]<
                    $($tbl: Table,)+
                    HashTbl: Table,
                    Tx: for<'tx> DbTx<'tx>
                >(
                    tx: &Tx,
                    $([< $tbl _range>]: impl RangeBounds<RawKey<<$tbl as Table>::Key>>,)+
                    compression_set: Option<Vec<impl Iterator<Item = Vec<u8>>>>,
                    hash_range: Option<impl RangeBounds<RawKey<<HashTbl as Table>::Key>>>,
                    row_count: usize,
                    nippy_jar: &mut NippyJar
                ) -> Result<(), DatabaseError> {

                    handle_compression_and_indexing::<HashTbl, Tx>(
                        tx, compression_set, hash_range, nippy_jar
                    )?;

                    // Creates the cursors for the columns
                    $(
                        let mut [< $tbl _cursor>] = tx.cursor_read::<RawTable<$tbl>>()?;
                        let [< $tbl _iter>] = [< $tbl _cursor>]
                            .walk_range([< $tbl _range>])?
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

fn handle_compression_and_indexing<HashTbl: Table, Tx: for<'tx> DbTx<'tx>>(
    tx: &Tx,
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
