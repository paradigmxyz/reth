use super::models::Test;
use crate::test_eth_chain::models::ForkSpec;
use reth_db::{
    database::Database,
    mdbx::{test_utils::create_test_rw_db, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_executor::SpecUpgrades;
use reth_primitives::{
    keccak256, Account as RethAccount, BigEndianHash, SealedBlock, SealedHeader, StorageEntry, H256,
};
use reth_rlp::Decodable;
use reth_stages::{stages::execution::ExecutionStage, ExecInput, Stage, Transaction};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};
use tracing::debug;

/// Tests are test edge cases that are not possible to happen on mainnet, so we are skipping them.
pub fn should_skip(path: &Path) -> bool {
    // funky test with `bigint 0x00` value in json :) not possible to happen on mainnet and require
    // custom json parser. https://github.com/ethereum/tests/issues/971
    if path.file_name() == Some(OsStr::new("ValueOverflow.json")) {
        return true
    }
    // txbyte is of type 02 and we dont parse tx bytes for this test to fail.
    if path.file_name() == Some(OsStr::new("typeTwoBerlin.json")) {
        return true
    }
    // Test checks if nonce overflows. We are handling this correctly but we are not parsing
    // exception in testsuite There are more nonce overflow tests that are in internal
    // call/create, and those tests are passing and are enabled.
    if path.file_name() == Some(OsStr::new("CreateTransactionHighNonce.json")) {
        return true
    }

    // Test check if gas price overflows, we handle this correctly but does not match tests specific
    // exception.
    if path.file_name() == Some(OsStr::new("HighGasPrice.json")) {
        return true
    }

    // Skip test where basefee/accesslist/diffuculty is present but it shouldn't be supported in
    // London/Berlin/TheMerge. https://github.com/ethereum/tests/blob/5b7e1ab3ffaf026d99d20b17bb30f533a2c80c8b/GeneralStateTests/stExample/eip1559.json#L130
    // It is expected to not execute these tests.
    if path.file_name() == Some(OsStr::new("accessListExample.json")) ||
        path.file_name() == Some(OsStr::new("basefeeExample.json")) ||
        path.file_name() == Some(OsStr::new("eip1559.json")) ||
        path.file_name() == Some(OsStr::new("mergeTest.json"))
    {
        return true
    }

    // These tests are passing, but they take a lot of time to execute so we are going to skip them.
    if path.file_name() == Some(OsStr::new("loopExp.json")) ||
        path.file_name() == Some(OsStr::new("Call50000_sha256.json")) ||
        path.file_name() == Some(OsStr::new("static_Call50000_sha256.json")) ||
        path.file_name() == Some(OsStr::new("loopMul.json")) ||
        path.file_name() == Some(OsStr::new("CALLBlake2f_MaxRounds.json"))
    {
        return true
    }
    false
}

/// Run one JSON-encoded Ethereum blockchain test at the specified path.
pub async fn run_test(path: PathBuf) -> eyre::Result<()> {
    let path = path.as_path();
    let json_file = std::fs::read(path)?;
    let suites: Test = serde_json::from_reader(&*json_file)?;

    if should_skip(path) {
        return Ok(())
    }

    for (name, suite) in suites.0 {
        if matches!(
            suite.network,
            ForkSpec::ByzantiumToConstantinopleAt5 |
                ForkSpec::Constantinople |
                ForkSpec::MergeEOF |
                ForkSpec::MergeMeterInitCode
        ) {
            continue
        }

        // if matches!(suite.pre, State(RootOrState::Root(_))) {}

        let pre_state = suite.pre.0;

        debug!("Executing test: {name} for spec: {:?}", suite.network);

        let spec_upgrades: SpecUpgrades = suite.network.into();
        // if paris aka merge is not activated we dont have block rewards;
        let has_block_reward = spec_upgrades.paris != 0;

        // Create db and acquire transaction
        let db = create_test_rw_db::<WriteMap>();
        let tx = db.tx_mut()?;

        // insert genesis
        let header: SealedHeader = suite.genesis_block_header.into();
        let genesis_block = SealedBlock { header, body: vec![], ommers: vec![] };
        reth_provider::insert_canonical_block(&tx, &genesis_block, has_block_reward)?;

        suite.blocks.iter().try_for_each(|block| -> eyre::Result<()> {
            let decoded = SealedBlock::decode(&mut block.rlp.as_ref())?;
            reth_provider::insert_canonical_block(&tx, &decoded, has_block_reward)?;
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
        // Hardcode the chain_id to Ethereums 1.
        let mut stage =
            ExecutionStage::new(reth_executor::Config { chain_id: 1.into(), spec_upgrades });

        // Call execution stage
        let input = ExecInput::default();
        stage.execute(&mut Transaction::new(db.as_ref())?, input).await?;

        // Validate post state
        //for post in
    }
    Ok(())
}
