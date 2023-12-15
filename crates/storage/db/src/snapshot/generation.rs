use crate::{
    abstraction::cursor::DbCursorRO,
    table::{Key, Table},
    transaction::DbTx,
    RawKey, RawTable,
};

use reth_interfaces::provider::ProviderResult;
use reth_nippy_jar::{ColumnResult, NippyJar, NippyJarHeader, PHFKey};
use reth_tracing::tracing::*;
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
                    H: NippyJarHeader
                >
                (
                    tx: &impl DbTx,
                    range: RangeInclusive<K>,
                    additional: Option<Vec<Box<dyn Iterator<Item = Result<Vec<u8>, Box<dyn StdError + Send + Sync>>>>>>,
                    dict_compression_set: Option<Vec<impl Iterator<Item = Vec<u8>>>>,
                    keys: Option<impl Iterator<Item = ColumnResult<impl PHFKey>>>,
                    row_count: usize,
                    nippy_jar: &mut NippyJar<H>
                ) -> ProviderResult<()>
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
