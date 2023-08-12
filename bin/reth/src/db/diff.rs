use std::{
    collections::HashMap,
    fmt::Debug,
    fs::{self, File},
    hash::Hash,
    io::Write,
    path::{Path, PathBuf},
};

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
use tracing::info;

#[derive(Parser, Debug)]
/// The arguments for the `reth db diff` command
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    #[arg(long, verbatim_doc_comment)]
    secondary_datadir: PlatformPath<DataDirPath>,

    /// Arguments for the second database
    #[clap(flatten)]
    second_db: DatabaseArgs,

    /// The table name to diff. If not specified, all tables are diffed.
    #[arg(long, verbatim_doc_comment)]
    table: Option<Tables>,

    /// The output directory for the diff report.
    #[arg(long, verbatim_doc_comment)]
    output: PlatformPath<PathBuf>,
}

impl Command {
    /// Execute the `db diff` command.
    ///
    /// This first opens the `db/` folder from the secondary datadir, where the second database is
    /// opened read-only.
    ///
    /// The tool will then iterate through all key-value pairs for the primary and secondary
    /// databases. The value for each key will be compared with its corresponding value in the
    /// other database. If the values are different, a discrepancy will be recorded in-memory. If
    /// one key is present in one database but not the other, this will be recorded as an "extra
    /// element" for that database.
    ///
    /// The discrepancies and extra elements, along with a brief summary of the diff results are
    /// then written to a file in the output directory.
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

            let output_dir = self.output.clone();
            match table {
                Tables::CanonicalHeaders => {
                    find_diffs::<CanonicalHeaders>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::HeaderTD => find_diffs::<HeaderTD>(primary_tx, secondary_tx, output_dir)?,
                Tables::HeaderNumbers => {
                    find_diffs::<HeaderNumbers>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::Headers => find_diffs::<Headers>(primary_tx, secondary_tx, output_dir)?,
                Tables::BlockBodyIndices => {
                    find_diffs::<BlockBodyIndices>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::BlockOmmers => {
                    find_diffs::<BlockOmmers>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::BlockWithdrawals => {
                    find_diffs::<BlockWithdrawals>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::TransactionBlock => {
                    find_diffs::<TransactionBlock>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::Transactions => {
                    find_diffs::<Transactions>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::TxHashNumber => {
                    find_diffs::<TxHashNumber>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::Receipts => find_diffs::<Receipts>(primary_tx, secondary_tx, output_dir)?,
                Tables::PlainAccountState => {
                    find_diffs::<PlainAccountState>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::PlainStorageState => {
                    find_diffs::<PlainStorageState>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::Bytecodes => find_diffs::<Bytecodes>(primary_tx, secondary_tx, output_dir)?,
                Tables::AccountHistory => {
                    find_diffs::<AccountHistory>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::StorageHistory => {
                    find_diffs::<StorageHistory>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::AccountChangeSet => {
                    find_diffs::<AccountChangeSet>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::StorageChangeSet => {
                    find_diffs::<StorageChangeSet>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::HashedAccount => {
                    find_diffs::<HashedAccount>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::HashedStorage => {
                    find_diffs::<HashedStorage>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::AccountsTrie => {
                    find_diffs::<AccountsTrie>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::StoragesTrie => {
                    find_diffs::<StoragesTrie>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::TxSenders => find_diffs::<TxSenders>(primary_tx, secondary_tx, output_dir)?,
                Tables::SyncStage => find_diffs::<SyncStage>(primary_tx, secondary_tx, output_dir)?,
                Tables::SyncStageProgress => {
                    find_diffs::<SyncStageProgress>(primary_tx, secondary_tx, output_dir)?
                }
                Tables::PruneCheckpoints => {
                    find_diffs::<PruneCheckpoints>(primary_tx, secondary_tx, output_dir)?
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
    output_dir: impl AsRef<Path>,
) -> eyre::Result<()>
where
    T::Key: Hash,
    T::Value: PartialEq,
{
    let table_name = T::NAME;

    info!("Analyzing table {table_name}...");
    let result = find_diffs_advanced::<T>(&primary_tx, &secondary_tx)?;
    info!("Done analyzing table {table_name}!");

    // Pretty info summary header: newline then header
    info!("");
    info!("Diff results for {table_name}:");

    // create directory and open file
    fs::create_dir_all(output_dir.as_ref())?;
    let file_name = format!("{table_name}.txt");
    let mut file = File::create(output_dir.as_ref().join(file_name.clone()))?;

    // analyze the result and print some stats
    let discrepancies = result.discrepancies.len();
    let extra_elements = result.extra_elements.len();

    // Make a pretty summary header for the table
    writeln!(file, "Diff results for {table_name}")?;

    if discrepancies > 0 {
        // write to file
        writeln!(file, "Found {discrepancies} discrepancies in table {table_name}")?;

        // also print to info
        info!("Found {discrepancies} discrepancies in table {table_name}");
    } else {
        // write to file
        writeln!(file, "No discrepancies found in table {table_name}")?;

        // also print to info
        info!("No discrepancies found in table {table_name}");
    }

    if extra_elements > 0 {
        // write to file
        writeln!(file, "Found {extra_elements} extra elements in table {table_name}")?;

        // also print to info
        info!("Found {extra_elements} extra elements in table {table_name}");
    } else {
        writeln!(file, "No extra elements found in table {table_name}")?;

        // also print to info
        info!("No extra elements found in table {table_name}");
    }

    info!("Writing diff results for {table_name} to {file_name}...");

    if discrepancies > 0 {
        writeln!(file, "Discrepancies:")?;
    }

    for discrepancy in result.discrepancies.values() {
        writeln!(file, "{discrepancy:?}")?;
    }

    if extra_elements > 0 {
        writeln!(file, "Extra elements:")?;
    }

    for extra_element in result.extra_elements.values() {
        writeln!(file, "{extra_element:?}")?;
    }

    let full_file_name = output_dir.as_ref().join(file_name);
    info!("Done writing diff results for {table_name} to {}", full_file_name.display());
    Ok(())
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

    /// The element from the first table
    #[allow(dead_code)]
    first: T::Value,

    /// The element from the second table
    #[allow(dead_code)]
    second: T::Value,
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
                    self.push_discrepancy(TableDiffElement { key, first, second });
                }
            }
            (Some(first), None) => {
                self.push_extra_element(ExtraTableElement::First { key, value: first });
            }
            (None, Some(second)) => {
                self.push_extra_element(ExtraTableElement::Second { key, value: second });
            }
            (None, None) => {}
        }
    }
}

/// A single extra element from a table
#[derive(Debug)]
enum ExtraTableElement<T: Table> {
    /// The extra element that is in the first table
    #[allow(dead_code)]
    First { key: T::Key, value: T::Value },

    /// The extra element that is in the second table
    #[allow(dead_code)]
    Second { key: T::Key, value: T::Value },
}

impl<T: Table> ExtraTableElement<T> {
    /// Return the key for the extra element
    fn key(&self) -> &T::Key {
        match self {
            Self::First { key, .. } => key,
            Self::Second { key, .. } => key,
        }
    }
}
