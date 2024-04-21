#![allow(unreachable_pub)]

use super::constants;
use reth_db::{
    cursor::DbCursorRO, database::Database, tables, transaction::DbTx, DatabaseError as DbError,
};
use reth_primitives::{fs, stage::StageCheckpoint, BlockNumber};
use reth_stages::{
    stages::{AccountHashingStage, SeedOpts},
    test_utils::TestStageDB,
};
use std::{ops::RangeInclusive, path::Path};

/// Prepares a database for [`AccountHashingStage`]
/// If the environment variable [`constants::ACCOUNT_HASHING_DB`] is set, it will use that one and
/// will get the stage execution range from [`tables::BlockBodyIndices`]. Otherwise, it will
/// generate its own random data.
///
/// Returns the path to the database file, stage and range of stage execution if it exists.
pub fn prepare_account_hashing(
    num_blocks: u64,
) -> (TestStageDB, AccountHashingStage, RangeInclusive<BlockNumber>) {
    let (db, stage_range) = match std::env::var(constants::ACCOUNT_HASHING_DB) {
        Ok(db) => {
            let path = Path::new(&db).to_path_buf();
            let range = find_stage_range(&path);
            (TestStageDB::new(&path), range)
        }
        Err(_) => generate_testdata_db(num_blocks),
    };

    (db, AccountHashingStage::default(), stage_range)
}

fn find_stage_range(db: &Path) -> RangeInclusive<BlockNumber> {
    let mut stage_range = None;
    TestStageDB::new(db)
        .factory
        .db_ref()
        .view(|tx| {
            let mut cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
            let from = cursor.first()?.unwrap().0;
            let to = StageCheckpoint::new(cursor.last()?.unwrap().0);

            stage_range = Some(from..=to.block_number);
            Ok::<(), DbError>(())
        })
        .unwrap()
        .unwrap();

    stage_range.expect("Could not find the stage range from the external DB.")
}

fn generate_testdata_db(num_blocks: u64) -> (TestStageDB, RangeInclusive<BlockNumber>) {
    let opts = SeedOpts { blocks: 0..=num_blocks, accounts: 100_000, txs: 100..150 };

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("account-hashing-bench");
    let exists = path.exists();
    let db = TestStageDB::new(&path);

    if !exists {
        // create the dirs
        fs::create_dir_all(&path).unwrap();
        println!("Account Hashing testdata not found, generating to {:?}", path.display());
        let provider = db.factory.provider_rw().unwrap();
        let _accounts = AccountHashingStage::seed(&provider, opts.clone());
        provider.commit().expect("failed to commit");
    }

    (db, opts.blocks)
}
