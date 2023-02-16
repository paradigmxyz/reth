use reth_interfaces::test_utils::generators::random_block_range;
use reth_primitives::H256;
use reth_stages::{
    stages::{AccountHashingStage, SeedOpts},
    test_utils::TestTransaction,
};
use std::path::{Path, PathBuf};

// Helper for generating testdata for the sender recovery stage and tx lookup stages (512MB).
// Returns the path to the database file and the number of blocks written.
pub fn txs_testdata(num_blocks: usize) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("txs-bench");
    let txs_range = 100..150;

    if !path.exists() {
        // create the dirs
        std::fs::create_dir_all(&path).unwrap();
        println!("Transactions testdata not found, generating to {:?}", path.display());
        let tx = TestTransaction::new(&path);

        // This takes a while because it does sig recovery internally
        let blocks = random_block_range(0..num_blocks as u64 + 1, H256::zero(), txs_range);

        // insert all blocks
        tx.insert_blocks(blocks.iter(), None).unwrap();

        // // initialize TD
        use reth_db::{
            cursor::DbCursorRO,
            tables,
            transaction::{DbTx, DbTxMut},
        };
        tx.commit(|tx| {
            let (head, _) = tx.cursor_read::<tables::Headers>()?.first()?.unwrap_or_default();
            tx.put::<tables::HeaderTD>(head, reth_primitives::U256::from(0).into())
        })
        .unwrap();
    }

    path
}

// Helper for generating testdata for the account hashing benchmark
// Returns the path to the database file and the stage.
pub fn account_hashing(num_blocks: u64) -> (PathBuf, AccountHashingStage) {
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

    let stage = AccountHashingStage::default();

    (path, stage)
}
