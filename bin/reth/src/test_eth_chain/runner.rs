use super::models::Test;
use std::path::Path;
use reth_db::database::Database;
use reth_primitives::BlockLocked;
use tracing::debug;

 
/// Run one JSON-encoded Ethereum blockchain test at the specified path.
pub async fn run_test(path: &Path) -> eyre::Result<()> {
    let json_file = std::fs::read(path)?;
    let suits: Test = serde_json::from_reader(&*json_file)?;

    for (name,suit) in suits.0 {
        debug!("Executing test:{name}");
        let mut db = reth_db::mdbx::test_utils::create_test_rw_db();
        let tx = db.tx_mut()();

        // insert genesis
        let genesis_block = BlockLocked {
            header: suit.genesis_block_header,
            body: vec![],
            ommers: vec![],
        };
        reth_provider::insert_canonical_block(tx,  &genesis_block, true)?;

        let blocks = suit.blocks.into_iter().map(|b| BlockLocked::decode(b.rlp)


    }
    Ok(())
}
