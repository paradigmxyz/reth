use crate::test_eth_chain::models::{RootOrState, State};

use super::models::Test;
use reth_db::{
    database::Database,
    mdbx::{test_utils::create_test_rw_db, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    keccak256, Account as RethAccount, BigEndianHash, BlockLocked, SealedHeader, StorageEntry, H256,
};
use reth_rlp::Decodable;
use reth_stages::{stages::execution::ExecutionStage, ExecInput, Stage, StageDB};
use std::path::Path;
use tracing::debug;

/// Run one JSON-encoded Ethereum blockchain test at the specified path.
pub async fn run_test(path: &Path) -> eyre::Result<()> {
    let json_file = std::fs::read(path)?;
    let suites: Test = serde_json::from_reader(&*json_file)?;

    for (name, suite) in suites.0 {
        if matches!(suite.pre, State(RootOrState::Root(_))) {}

        let pre_state = match suite.pre.0 {
            RootOrState::State(state) => state,
            RootOrState::Root(_) => {
                debug!("Skipping test {name}...");
                continue
            }
        };

        debug!("Executing test: {name}");

        // Create db and acquire transaction
        let db = create_test_rw_db::<WriteMap>();
        let tx = db.tx_mut()?;

        // insert genesis
        let header: SealedHeader = suite.genesis_block_header.into();
        let genesis_block = BlockLocked { header, body: vec![], ommers: vec![] };
        reth_provider::insert_canonical_block(&tx, &genesis_block, true)?;

        suite.blocks.iter().try_for_each(|block| -> eyre::Result<()> {
            let decoded = BlockLocked::decode(&mut block.rlp.as_ref())?;
            reth_provider::insert_canonical_block(&tx, &decoded, true)?;
            Ok(())
        })?;

        pre_state.into_iter().try_for_each(|(address, account)| -> eyre::Result<()> {
            let has_code = !account.code.is_empty();
            let code_hash = if has_code { Some(keccak256(&account.code)) } else { None };
            tx.put::<tables::PlainAccountState>(
                address,
                RethAccount {
                    balance: account.balance.0,
                    nonce: account.nonce.0.as_u64(),
                    bytecode_hash: code_hash,
                },
            )?;
            if let Some(code_hash) = code_hash {
                tx.put::<tables::Bytecodes>(code_hash, account.code.to_vec())?;
            }
            account.storage.iter().try_for_each(|(k, v)| {
                tx.put::<tables::PlainStorageState>(
                    address,
                    StorageEntry { key: H256::from_uint(&k.0), value: v.0 },
                )
            })?;

            Ok(())
        })?;

        // Commit the pre suite state
        tx.commit()?;

        // Initialize the execution stage
        let mut stage = ExecutionStage::default(); // TODO: review chain config

        // Call execution stage
        let input = ExecInput::default(); // TODO:
        stage.execute(&mut StageDB::new(db.as_ref())?, input).await?;

        // Validate post state
        // TODO:
    }
    Ok(())
}
