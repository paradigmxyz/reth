use bincode::serialize_into;
use eyre::Result;
use proptest::{
    arbitrary::Arbitrary,
    prelude::{any_with, ProptestConfig},
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_db::{table::Table, tables};

const VECTORS_FOLDER: &str = "test-vectors";

pub(crate) fn generate_db_vectors() -> Result<()> {
    let mut runner = TestRunner::new(ProptestConfig::default());
    std::fs::create_dir_all(VECTORS_FOLDER)?;

    generate_table_vectors::<tables::CanonicalHeaders>(&mut runner)?;
    generate_table_vectors::<tables::HeaderTD>(&mut runner)?;
    generate_table_vectors::<tables::HeaderNumbers>(&mut runner)?;
    generate_table_vectors::<tables::Headers>(&mut runner)?;
    generate_table_vectors::<tables::BlockBodies>(&mut runner)?;
    generate_table_vectors::<tables::BlockOmmers>(&mut runner)?;
    generate_table_vectors::<tables::TxHashNumber>(&mut runner)?;
    generate_table_vectors::<tables::BlockTransitionIndex>(&mut runner)?;
    generate_table_vectors::<tables::TxTransitionIndex>(&mut runner)?;
    generate_table_vectors::<tables::SyncStage>(&mut runner)?;
    generate_table_vectors::<tables::Transactions>(&mut runner)?;
    generate_table_vectors::<tables::PlainStorageState>(&mut runner)?;
    generate_table_vectors::<tables::PlainAccountState>(&mut runner)?;

    Ok(())
}

fn generate_table_vectors<T: Table>(runner: &mut TestRunner) -> Result<()>
where
    T::Key: Arbitrary + serde::Serialize + Ord,
    T::Value: Arbitrary + serde::Serialize,
{
    println!("Generating test vectors for <{}>.", T::NAME);

    let per_table = 100;
    let strategy = proptest::collection::vec(
        any_with::<(T::Key, T::Value)>((
            <T::Key as Arbitrary>::Parameters::default(),
            <T::Value as Arbitrary>::Parameters::default(),
        )),
        per_table,
    )
    .no_shrink()
    .boxed();

    // Generate all `per_table` rows: (Key, Value)
    let mut rows = strategy.new_tree(runner).map_err(|e| eyre::eyre!("{e}"))?.current();

    // Sort them by `Key`
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    // Save them to file
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(format!("{VECTORS_FOLDER}/{}", T::NAME)).unwrap(),
    );
    serialize_into(&mut f, &rows)?;

    Ok(())
}
