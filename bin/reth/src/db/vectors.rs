use eyre::Result;
use proptest::{
    arbitrary::Arbitrary,
    prelude::{any_with, ProptestConfig},
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_db::{table::Table, tables};

const VECTORS_FOLDER: &str = "testdata/micro/db";

pub(crate) fn generate_db_vectors() -> Result<()> {
    let mut runner = TestRunner::new(ProptestConfig::default());
    std::fs::create_dir_all(VECTORS_FOLDER)?;

    let per_table = 1000;

    generate_table_vectors::<tables::CanonicalHeaders>(&mut runner, per_table)?;
    generate_table_vectors::<tables::HeaderTD>(&mut runner, per_table)?;
    generate_table_vectors::<tables::HeaderNumbers>(&mut runner, per_table)?;
    generate_table_vectors::<tables::Headers>(&mut runner, per_table)?;
    generate_table_vectors::<tables::BlockBodies>(&mut runner, per_table)?;
    generate_table_vectors::<tables::BlockOmmers>(&mut runner, 100)?;
    generate_table_vectors::<tables::TxHashNumber>(&mut runner, per_table)?;
    generate_table_vectors::<tables::BlockTransitionIndex>(&mut runner, per_table)?;
    generate_table_vectors::<tables::TxTransitionIndex>(&mut runner, per_table)?;
    generate_table_vectors::<tables::Transactions>(&mut runner, 100)?;
    generate_table_vectors::<tables::PlainStorageState>(&mut runner, per_table)?;
    generate_table_vectors::<tables::PlainAccountState>(&mut runner, per_table)?;

    Ok(())
}

fn generate_table_vectors<T: Table>(runner: &mut TestRunner, per_table: usize) -> Result<()>
where
    T::Key: Arbitrary + serde::Serialize + Ord,
    T::Value: Arbitrary + serde::Serialize,
{
    println!("Generating test vectors for <{}>.", T::NAME);

    let mut rows = vec![];

    // We don't want repeated keys
    let mut seen_keys = vec![];

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
                .filter(|e| {
                    if seen_keys.contains(&e.0) {
                        return false
                    }
                    seen_keys.push(e.0.clone());
                    true
                }),
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
