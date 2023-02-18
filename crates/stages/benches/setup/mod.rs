use reth_db::{
    cursor::DbCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::test_utils::generators::random_block_range;
use reth_primitives::H256;
use reth_stages::test_utils::TestTransaction;
use std::path::{Path, PathBuf};

mod constants;

mod account_hashing;
pub use account_hashing::*;

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

        // initialize TD
        tx.commit(|tx| {
            let (head, _) = tx.cursor_read::<tables::Headers>()?.first()?.unwrap_or_default();
            tx.put::<tables::HeaderTD>(head, reth_primitives::U256::from(0).into())
        })
        .unwrap();
    }

    path
}
