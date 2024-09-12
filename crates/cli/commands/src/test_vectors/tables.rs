use alloy_primitives::private::getrandom::getrandom;
use arbitrary::Arbitrary;
use eyre::Result;
use proptest::{
    prelude::ProptestConfig,
    strategy::{Strategy, ValueTree},
    test_runner::{TestRng, TestRunner},
};
use proptest_arbitrary_interop::arb;
use reth_db::tables;
use reth_db_api::table::{DupSort, Table, TableRow};
use reth_fs_util as fs;
use std::collections::HashSet;
use tracing::error;

const VECTORS_FOLDER: &str = "testdata/micro/db";
const PER_TABLE: usize = 1000;

/// Generates test vectors for specified `tables`. If list is empty, then generate for all tables.
pub(crate) fn generate_vectors(mut tables: Vec<String>) -> Result<()> {
    // Prepare random seed for test (same method as used by proptest)
    let mut seed = [0u8; 32];
    getrandom(&mut seed)?;
    println!("Seed for test vectors: {:?}", seed);

    // Start the runner with the seed
    let config = ProptestConfig::default();
    let rng = TestRng::from_seed(config.rng_algorithm, &seed);
    let mut runner = TestRunner::new_with_rng(config, rng);

    fs::create_dir_all(VECTORS_FOLDER)?;

    macro_rules! generate_vector {
        ($table_type:ident, $per_table:expr, TABLE) => {
            generate_table_vector::<tables::$table_type>(&mut runner, $per_table)?;
        };
        ($table_type:ident, $per_table:expr, DUPSORT) => {
            generate_dupsort_vector::<tables::$table_type>(&mut runner, $per_table)?;
        };
    }

    macro_rules! generate {
        ([$(($table_type:ident, $per_table:expr, $table_or_dup:tt)),*]) => {
            let all_tables = vec![$(stringify!($table_type).to_string(),)*];

            if tables.is_empty() {
                tables = all_tables;
            }

            for table in tables {
                match table.as_str() {
                    $(
                        stringify!($table_type) => {
                            println!("Generating test vectors for {} <{}>.", stringify!($table_or_dup), tables::$table_type::NAME);

                            generate_vector!($table_type, $per_table, $table_or_dup);
                        },
                    )*
                    _ => {
                        error!(target: "reth::cli", "Unknown table: {}", table);
                    }
                }
            }
        }
    }

    generate!([
        (CanonicalHeaders, PER_TABLE, TABLE),
        (HeaderTerminalDifficulties, PER_TABLE, TABLE),
        (HeaderNumbers, PER_TABLE, TABLE),
        (Headers, PER_TABLE, TABLE),
        (BlockBodyIndices, PER_TABLE, TABLE),
        (BlockOmmers, 100, TABLE),
        (TransactionHashNumbers, PER_TABLE, TABLE),
        (Transactions, 100, TABLE),
        (PlainStorageState, PER_TABLE, DUPSORT),
        (PlainAccountState, PER_TABLE, TABLE)
    ]);

    Ok(())
}

/// Generates test-vectors for normal tables. Keys are sorted and not repeated.
fn generate_table_vector<T>(runner: &mut TestRunner, per_table: usize) -> Result<()>
where
    T: Table,
    T::Key: for<'a> Arbitrary<'a> + serde::Serialize + Ord + std::hash::Hash + Clone,
    T::Value: for<'a> Arbitrary<'a> + serde::Serialize + Clone,
{
    let mut rows = vec![];
    let mut seen_keys = HashSet::new();
    let strategy =
        proptest::collection::vec(arb::<TableRow<T>>(), per_table - rows.len()).no_shrink().boxed();

    while rows.len() < per_table {
        // Generate all `per_table` rows: (Key, Value)
        rows.extend(
            &mut strategy
                .new_tree(runner)
                .map_err(|e| eyre::eyre!("{e}"))?
                .current()
                .into_iter()
                .filter(|e| seen_keys.insert(e.0.clone())),
        );
    }
    // Sort them by `Key`
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    save_to_file::<T>(rows)
}

/// Generates test-vectors for DUPSORT tables. Each key has multiple (subkey, value). Keys and
/// subkeys are sorted.
fn generate_dupsort_vector<T>(runner: &mut TestRunner, per_table: usize) -> Result<()>
where
    T: Table + DupSort,
    T::Key: for<'a> Arbitrary<'a> + serde::Serialize + Ord + std::hash::Hash + Clone,
    T::Value: for<'a> Arbitrary<'a> + serde::Serialize + Ord + Clone,
{
    let mut rows = vec![];

    // We want to control our repeated keys
    let mut seen_keys = HashSet::new();

    let strat_values = proptest::collection::vec(arb::<T::Value>(), 100..300).no_shrink().boxed();

    let strat_keys = arb::<T::Key>().no_shrink().boxed();

    while rows.len() < per_table {
        let key: T::Key = strat_keys.new_tree(runner).map_err(|e| eyre::eyre!("{e}"))?.current();

        if !seen_keys.insert(key.clone()) {
            continue
        }

        let mut values: Vec<T::Value> =
            strat_values.new_tree(runner).map_err(|e| eyre::eyre!("{e}"))?.current();

        values.sort();

        for value in values {
            rows.push((key.clone(), value));
        }
    }

    // Sort them by `Key`
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    save_to_file::<T>(rows)
}

/// Save rows to file.
fn save_to_file<T: Table>(rows: Vec<TableRow<T>>) -> eyre::Result<()>
where
    T::Key: serde::Serialize,
    T::Value: serde::Serialize,
{
    serde_json::to_writer_pretty(
        std::io::BufWriter::new(
            std::fs::File::create(format!("{VECTORS_FOLDER}/{}.json", T::NAME)).unwrap(),
        ),
        &rows,
    )
    .map_err(|e| eyre::eyre!({ e }))
}
