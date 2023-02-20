use super::constants;
use reth_db::{
    cursor::DbCursorRO, database::Database, tables, transaction::DbTx, Error as DbError,
};
use reth_stages::{
    stages::{AccountHashingStage, SeedOpts},
    test_utils::TestTransaction,
    ExecInput, StageId, UnwindInput,
};
use std::path::{Path, PathBuf};

/// Prepares a database for [`AccountHashingStage`]
/// If the environment variable [`constants::ACCOUNT_HASHING_DB`] is set, it will use that one and
/// will get the stage execution range from [`tables::BlockTransitionIndex`]. Otherwise, it will
/// generate its own random data.
///
/// Returns the path to the database file, stage and range of stage execution if it exists.
pub fn prepare_account_hashing(
    num_blocks: u64,
) -> (PathBuf, AccountHashingStage, (ExecInput, UnwindInput)) {
    let (path, stage_range) = match std::env::var(constants::ACCOUNT_HASHING_DB) {
        Ok(db) => {
            let path = Path::new(&db).to_path_buf();
            let range = find_stage_range(&path);
            (path, range)
        }
        Err(_) => generate_testdata_db(num_blocks),
    };

    (path, AccountHashingStage::default(), stage_range)
}

fn find_stage_range(db: &Path) -> (ExecInput, UnwindInput) {
    let mut stage_range = None;
    TestTransaction::new(db)
        .tx
        .view(|tx| {
            let mut cursor = tx.cursor_read::<tables::BlockTransitionIndex>()?;
            let from = cursor.first()?.unwrap().0;
            let to = cursor.last()?.unwrap().0;

            stage_range = Some((
                ExecInput {
                    previous_stage: Some((StageId("Another"), to)),
                    stage_progress: Some(from),
                },
                UnwindInput { unwind_to: from, stage_progress: to, bad_block: None },
            ));
            Ok::<(), DbError>(())
        })
        .unwrap()
        .unwrap();

    stage_range.expect("Could not find the stage range from the external DB.")
}

fn generate_testdata_db(num_blocks: u64) -> (PathBuf, (ExecInput, UnwindInput)) {
    let opts = SeedOpts {
        blocks: 0..num_blocks + 1,
        accounts: 0..10_000,
        txs: 100..150,
        transitions: 10_000 + 1,
    };

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("account-hashing-bench");

    if !path.exists() {
        // create the dirs
        std::fs::create_dir_all(&path).unwrap();
        println!("Account Hashing testdata not found, generating to {:?}", path.display());
        let tx = TestTransaction::new(&path);
        let mut tx = tx.inner();
        let _accounts = AccountHashingStage::seed(&mut tx, opts);
    }
    (
        path,
        (
            ExecInput {
                previous_stage: Some((StageId("Another"), num_blocks)),
                ..Default::default()
            },
            UnwindInput::default(),
        ),
    )
}
