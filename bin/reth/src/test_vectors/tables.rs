use std::collections::HashSet;

use eyre::Result;
use proptest::{
    arbitrary::Arbitrary,
    prelude::{any_with, ProptestConfig},
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_db::{table::Table, tables};
use tracing::error;

const VECTORS_FOLDER: &str = "testdata/micro/db";
const PER_TABLE: usize = 1000;

/// Generates test vectors for specified `tables`. If list is empty, then generate for all tables.
pub(crate) fn generate_vectors(mut tables: Vec<String>) -> Result<()> {
    let mut runner = TestRunner::new(ProptestConfig::default());
    std::fs::create_dir_all(VECTORS_FOLDER)?;

    macro_rules! generate {
        ([$(($table_type:ident, $per_table:expr)),*]) => {
            let all_tables = vec![$(stringify!($table_type).to_string(),)*];

            if tables.is_empty() {
                tables = all_tables;
            }

            for table in tables {
                match table.as_str() {
                    $(
                        stringify!($table_type) => {
                            generate_table_vector::<tables::$table_type>(&mut runner, $per_table)?;
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
        (CanonicalHeaders, PER_TABLE),
        (HeaderTD, PER_TABLE),
        (HeaderNumbers, PER_TABLE),
        (Headers, PER_TABLE),
        (BlockBodies, PER_TABLE),
        (BlockOmmers, 100),
        (TxHashNumber, PER_TABLE),
        (BlockTransitionIndex, PER_TABLE),
        (TxTransitionIndex, PER_TABLE),
        (Transactions, 100),
        (PlainStorageState, PER_TABLE),
        (PlainAccountState, PER_TABLE)
    ]);

    Ok(())
}

fn generate_table_vector<T: Table>(runner: &mut TestRunner, per_table: usize) -> Result<()>
where
    T::Key: Arbitrary + serde::Serialize + Ord + std::hash::Hash,
    T::Value: Arbitrary + serde::Serialize,
{
    println!("Generating test vectors for <{}>.", T::NAME);

    let mut rows = vec![];

    // We don't want repeated keys
    let mut seen_keys = HashSet::new();

    while rows.len() < per_table {
        let strategy = proptest::collection::vec(
            any_with::<(T::Key, T::Value)>((
                <T::Key as Arbitrary>::Parameters::default(),
                <T::Value as Arbitrary>::Parameters::default(),
            )),
            per_table - rows.len(),
        )
        .no_shrink()
        .boxed();

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

    // Save them to file
    serde_json::to_writer_pretty(
        std::io::BufWriter::new(
            std::fs::File::create(format!("{VECTORS_FOLDER}/{}.json", T::NAME)).unwrap(),
        ),
        &rows,
    )
    .map_err(|e| eyre::eyre!({ e }))
}
