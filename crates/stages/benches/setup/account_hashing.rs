use super::{constants, StageRange};
use reth_db::{
    cursor::DbCursorRO, database::Database, tables, transaction::DbTx, DatabaseError as DbError,
};
use reth_primitives::stage::StageCheckpoint;
use reth_stages::{
    stages::{AccountHashingStage, SeedOpts},
    test_utils::TestTransaction,
    ExecInput, UnwindInput,
};
use std::path::{Path, PathBuf};

/// Prepares a database for [`AccountHashingStage`]
/// If the environment variable [`constants::ACCOUNT_HASHING_DB`] is set, it will use that one and
/// will get the stage execution range from [`tables::BlockBodyIndices`]. Otherwise, it will
/// generate its own random data.
///
/// Returns the path to the database file, stage and range of stage execution if it exists.
pub fn prepare_account_hashing(num_blocks: u64) -> (PathBuf, AccountHashingStage, StageRange) {
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

fn find_stage_range(db: &Path) -> StageRange {
    let mut stage_range = None;
    TestTransaction::new(db)
        .tx
        .view(|tx| {
            let mut cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
            let from = cursor.first()?.unwrap().0;
            let to = StageCheckpoint::new(cursor.last()?.unwrap().0);

            stage_range = Some((
                ExecInput {
                    target: Some(to.block_number),
                    checkpoint: Some(StageCheckpoint::new(from)),
                },
                UnwindInput { unwind_to: from, checkpoint: to, bad_block: None },
            ));
            Ok::<(), DbError>(())
        })
        .unwrap()
        .unwrap();

    stage_range.expect("Could not find the stage range from the external DB.")
}

fn generate_testdata_db(num_blocks: u64) -> (PathBuf, StageRange) {
    let opts = SeedOpts { blocks: 0..=num_blocks, accounts: 0..100_000, txs: 100..150 };

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("account-hashing-bench");

    if !path.exists() {
        // create the dirs
        std::fs::create_dir_all(&path).unwrap();
        println!("Account Hashing testdata not found, generating to {:?}", path.display());
        let tx = TestTransaction::new(&path);
        let mut tx = tx.inner();
        let _accounts = AccountHashingStage::seed(&mut tx, opts);
    }
    (path, (ExecInput { target: Some(num_blocks), ..Default::default() }, UnwindInput::default()))
}
