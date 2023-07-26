use std::{collections::HashMap, fmt::Debug, hash::Hash, path::PathBuf};

use crate::{
    args::DatabaseArgs,
    dirs::{DataDirPath, PlatformPath},
    utils::DbTool,
};
use clap::Parser;

use reth_db::{
    cursor::DbCursorRO, database::Database, open_db_read_only, table::Table, transaction::DbTx,
    AccountChangeSet, AccountHistory, AccountsTrie, BlockBodyIndices, BlockOmmers,
    BlockWithdrawals, Bytecodes, CanonicalHeaders, DatabaseEnvRO, HashedAccount, HashedStorage,
    HeaderNumbers, HeaderTD, Headers, PlainAccountState, PlainStorageState, PruneCheckpoints,
    Receipts, StorageChangeSet, StorageHistory, StoragesTrie, SyncStage, SyncStageProgress, Tables,
    TransactionBlock, Transactions, TxHashNumber, TxSenders,
};
use tracing::error;

#[derive(Parser, Debug)]
/// The arguments for the `reth db diff` command
pub struct Command {
    // THE SECOND DATABASE
    /// The path to the data dir for all reth files and subdirectories.
    #[arg(long, verbatim_doc_comment, global = true)]
    secondary_datadir: PlatformPath<DataDirPath>,

    /// Arguments for the second database
    #[clap(flatten)]
    second_db: DatabaseArgs,

    /// The table name to diff. If not specified, all tables are diffed.
    #[arg(long, verbatim_doc_comment)]
    table: Option<Tables>,
}

impl Command {
    /// Execute `db diff` command
    pub fn execute(self, tool: &DbTool<'_, DatabaseEnvRO>) -> eyre::Result<()> {
        // open second db
        let second_db_path: PathBuf = self.secondary_datadir.join("db").into();
        let second_db = open_db_read_only(&second_db_path, self.second_db.log_level)?;

        let tables = match self.table {
            Some(table) => vec![table],
            None => Tables::ALL.to_vec(),
        };

        for table in tables {
            let primary_tx = tool.db.tx()?;
            let secondary_tx = second_db.tx()?;

            match table {
                Tables::CanonicalHeaders => {
                    find_diffs::<CanonicalHeaders>(primary_tx, secondary_tx)?
                }
                Tables::HeaderTD => find_diffs::<HeaderTD>(primary_tx, secondary_tx)?,
                Tables::HeaderNumbers => find_diffs::<HeaderNumbers>(primary_tx, secondary_tx)?,
                Tables::Headers => find_diffs::<Headers>(primary_tx, secondary_tx)?,
                Tables::BlockBodyIndices => {
                    find_diffs::<BlockBodyIndices>(primary_tx, secondary_tx)?
                }
                Tables::BlockOmmers => find_diffs::<BlockOmmers>(primary_tx, secondary_tx)?,
                Tables::BlockWithdrawals => {
                    find_diffs::<BlockWithdrawals>(primary_tx, secondary_tx)?
                }
                Tables::TransactionBlock => {
                    find_diffs::<TransactionBlock>(primary_tx, secondary_tx)?
                }
                Tables::Transactions => find_diffs::<Transactions>(primary_tx, secondary_tx)?,
                Tables::TxHashNumber => find_diffs::<TxHashNumber>(primary_tx, secondary_tx)?,
                Tables::Receipts => find_diffs::<Receipts>(primary_tx, secondary_tx)?,
                Tables::PlainAccountState => {
                    find_diffs::<PlainAccountState>(primary_tx, secondary_tx)?
                }
                Tables::PlainStorageState => {
                    find_diffs::<PlainStorageState>(primary_tx, secondary_tx)?
                }
                Tables::Bytecodes => find_diffs::<Bytecodes>(primary_tx, secondary_tx)?,
                Tables::AccountHistory => find_diffs::<AccountHistory>(primary_tx, secondary_tx)?,
                Tables::StorageHistory => find_diffs::<StorageHistory>(primary_tx, secondary_tx)?,
                Tables::AccountChangeSet => {
                    find_diffs::<AccountChangeSet>(primary_tx, secondary_tx)?
                }
                Tables::StorageChangeSet => {
                    find_diffs::<StorageChangeSet>(primary_tx, secondary_tx)?
                }
                Tables::HashedAccount => find_diffs::<HashedAccount>(primary_tx, secondary_tx)?,
                Tables::HashedStorage => find_diffs::<HashedStorage>(primary_tx, secondary_tx)?,
                Tables::AccountsTrie => find_diffs::<AccountsTrie>(primary_tx, secondary_tx)?,
                Tables::StoragesTrie => find_diffs::<StoragesTrie>(primary_tx, secondary_tx)?,
                Tables::TxSenders => find_diffs::<TxSenders>(primary_tx, secondary_tx)?,
                Tables::SyncStage => find_diffs::<SyncStage>(primary_tx, secondary_tx)?,
                Tables::SyncStageProgress => {
                    find_diffs::<SyncStageProgress>(primary_tx, secondary_tx)?
                }
                Tables::PruneCheckpoints => {
                    find_diffs::<PruneCheckpoints>(primary_tx, secondary_tx)?
                }
            };
        }

        Ok(())
    }
}

/// Find diffs for a table, then analyzing the result
fn find_diffs<'a, T: Table>(
    primary_tx: impl DbTx<'a>,
    secondary_tx: impl DbTx<'a>,
) -> eyre::Result<()>
where
    T::Key: Hash,
    T::Value: PartialEq,
{
    let result = find_diffs_advanced::<T>(&primary_tx, &secondary_tx)?;

    // analyze the result and print some stats
    let discrepancies = result.discrepancies.len();
    let extra_elements = result.extra_elements.len();

    if discrepancies > 0 {
        error!("Found {} discrepancies in table {}", discrepancies, T::NAME);
    }

    if extra_elements > 0 {
        error!("Found {} extra elements in table {}", extra_elements, T::NAME);
    }

    for discrepancy in result.discrepancies {
        error!("Discrepancy: {:?}", discrepancy);
    }

    for extra_element in result.extra_elements {
        error!("Extra element: {:?}", extra_element);
    }

    Ok(())
}

/// Find diffs for a specific table. This will walk the first table, checking the second table
/// for each element. If the element is not found, it will be added to the extra elements set.
// TODO: remove this?
#[allow(dead_code)]
fn find_diffs_simple<'a, T: Table>(
    primary_tx: impl DbTx<'a>,
    secondary_tx: impl DbTx<'a>,
) -> eyre::Result<TableDiffResult<T>>
where
    T::Key: Hash,
    T::Value: PartialEq,
{
    // initialize the walker for the first table
    let mut primary_cursor =
        primary_tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");
    let primary_walker = primary_cursor.walk(None)?;

    let mut secondary_cursor =
        secondary_tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");
    let mut result = TableDiffResult::<T>::default();

    for entry in primary_walker {
        let (key, value) = entry?;
        let secondary_value = secondary_cursor.seek_exact(key.clone())?.map(|(_, value)| value);

        result.try_push_discrepancy(key, Some(value), secondary_value);
    }

    Ok(result)
}

/// This diff algorithm is slightly different, it will walk _each_ table, cross-checking for the
/// element in the other table.
fn find_diffs_advanced<'a, T: Table>(
    primary_tx: &impl DbTx<'a>,
    secondary_tx: &impl DbTx<'a>,
) -> eyre::Result<TableDiffResult<T>>
where
    T::Value: PartialEq,
    T::Key: Hash,
{
    // initialize the zipped walker
    let mut primary_zip_cursor =
        primary_tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");
    let primary_walker = primary_zip_cursor.walk(None)?;

    let mut secondary_zip_cursor =
        secondary_tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");
    let secondary_walker = secondary_zip_cursor.walk(None)?;
    let zipped_cursor = primary_walker.zip(secondary_walker);

    // initialize the cursors for seeking when we are cross checking elements
    let mut primary_cursor =
        primary_tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

    let mut secondary_cursor =
        secondary_tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

    let mut result = TableDiffResult::<T>::default();

    // this loop will walk both tables, cross-checking for the element in the other table.
    // it basically just loops through both tables at the same time. if the keys are different, it
    // will check each key in the other table. if the keys are the same, it will compare the
    // values
    for (primary_entry, secondary_entry) in zipped_cursor {
        let (primary_key, primary_value) = primary_entry?;
        let (secondary_key, secondary_value) = secondary_entry?;

        if primary_key != secondary_key {
            // if the keys are different, we need to check if the key is in the other table
            let crossed_secondary =
                secondary_cursor.seek_exact(primary_key.clone())?.map(|(_, value)| value);
            result.try_push_discrepancy(
                primary_key.clone(),
                Some(primary_value),
                crossed_secondary,
            );

            // now do the same for the primary table
            let crossed_primary =
                primary_cursor.seek_exact(secondary_key.clone())?.map(|(_, value)| value);
            result.try_push_discrepancy(
                secondary_key.clone(),
                crossed_primary,
                Some(secondary_value),
            );
        } else {
            // the keys are the same, so we need to compare the values
            result.try_push_discrepancy(primary_key, Some(primary_value), Some(secondary_value));
        }
    }

    Ok(result)
}

/// Includes a table element between two databases with the same key, but different values
#[derive(Debug)]
struct TableDiffElement<T: Table> {
    /// The key for the element
    key: T::Key,
    /// The element in the first table
    expected: T::Value,
    /// The element in the second table
    got: T::Value,
}

/// The diff result for an entire table. If the tables had the same number of elements, there will
/// be no extra elements.
struct TableDiffResult<T: Table>
where
    T::Key: Hash,
{
    /// All elements of the database that are different
    discrepancies: HashMap<T::Key, TableDiffElement<T>>,

    /// Any extra elements, and the table they are in
    extra_elements: HashMap<T::Key, ExtraTableElement<T>>,
}

impl<T> Default for TableDiffResult<T>
where
    T: Table,
    T::Key: Hash,
{
    fn default() -> Self {
        Self { discrepancies: HashMap::new(), extra_elements: HashMap::new() }
    }
}

impl<T: Table> TableDiffResult<T>
where
    T::Key: Hash,
{
    /// Push a diff result into the discrepancies set.
    fn push_discrepancy(&mut self, discrepancy: TableDiffElement<T>) {
        self.discrepancies.insert(discrepancy.key.clone(), discrepancy);
    }

    /// Push an extra element into the extra elements set.
    fn push_extra_element(&mut self, element: ExtraTableElement<T>) {
        self.extra_elements.insert(element.key().clone(), element);
    }
}

impl<T> TableDiffResult<T>
where
    T: Table,
    T::Key: Hash,
    T::Value: PartialEq,
{
    /// Try to push a diff result into the discrepancy set, only pushing if the given elements are
    /// different, and the discrepancy does not exist anywhere already.
    fn try_push_discrepancy(
        &mut self,
        key: T::Key,
        first: Option<T::Value>,
        second: Option<T::Value>,
    ) {
        // do not bother comparing if the key is already in the discrepancies map
        if self.discrepancies.contains_key(&key) {
            return
        }

        // do not bother comparing if the key is already in the extra elements map
        if self.extra_elements.contains_key(&key) {
            return
        }

        match (first, second) {
            (Some(first), Some(second)) => {
                if first != second {
                    self.push_discrepancy(TableDiffElement { key, expected: first, got: second });
                }
            }
            (Some(first), None) => {
                self.push_extra_element(ExtraTableElement::first(key, first));
            }
            (None, Some(second)) => {
                self.push_extra_element(ExtraTableElement::second(key, second));
            }
            (None, None) => {}
        }
    }
}

/// A single extra element from a table
#[derive(Debug)]
enum ExtraTableElement<T: Table> {
    /// The extra element is in the first table
    First { key: T::Key, value: T::Value },
    /// The extra element is in the second table
    Second { key: T::Key, value: T::Value },
}

impl<T: Table> ExtraTableElement<T> {
    /// Create a new extra element from the first table
    fn first(key: T::Key, value: T::Value) -> Self {
        Self::First { key, value }
    }

    /// Create a new extra element from the second table
    fn second(key: T::Key, value: T::Value) -> Self {
        Self::Second { key, value }
    }

    /// Return the key for the extra element
    fn key(&self) -> &T::Key {
        match self {
            Self::First { key, .. } => key,
            Self::Second { key, .. } => key,
        }
    }
}
