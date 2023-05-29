//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec, RootOrState},
    Case, Error, Suite,
};
use reth_db::mdbx::test_utils::create_test_rw_db;
use reth_primitives::{stage::StageId, BlockBody, SealedBlock, StageCheckpoint};
use reth_provider::Transaction;
use reth_stages::{stages::ExecutionStage, ExecInput, Stage};
use std::{collections::BTreeMap, ffi::OsStr, fs, ops::Deref, path::Path, sync::Arc};

/// A handler for the blockchain test suite.
#[derive(Debug)]
pub struct BlockchainTests {
    suite: String,
}

impl BlockchainTests {
    /// Create a new handler for a subset of the blockchain test suite.
    pub fn new(suite: String) -> Self {
        Self { suite }
    }
}

impl Suite for BlockchainTests {
    type Case = BlockchainTestCase;

    fn suite_name(&self) -> String {
        format!("BlockchainTests/{}", self.suite)
    }
}

/// An Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockchainTestCase {
    tests: BTreeMap<String, BlockchainTest>,
    skip: bool,
}

impl Case for BlockchainTestCase {
    fn load(path: &Path) -> Result<Self, Error> {
        Ok(BlockchainTestCase {
            tests: fs::read_to_string(path)
                .map_err(|e| Error::Io { path: path.into(), error: e.to_string() })
                .and_then(|s| {
                    serde_json::from_str(&s).map_err(|e| Error::CouldNotDeserialize {
                        path: path.into(),
                        error: e.to_string(),
                    })
                })?,
            skip: should_skip(path),
        })
    }

    // TODO: Clean up
    fn run(&self) -> Result<(), Error> {
        if self.skip {
            return Err(Error::Skipped)
        }

        for case in self.tests.values() {
            if matches!(
                case.network,
                ForkSpec::ByzantiumToConstantinopleAt5 |
                    ForkSpec::Constantinople |
                    ForkSpec::ConstantinopleFix |
                    ForkSpec::MergeEOF |
                    ForkSpec::MergeMeterInitCode |
                    ForkSpec::MergePush0 |
                    ForkSpec::Unknown
            ) {
                continue
            }

            // Create the database
            let db = create_test_rw_db();
            let mut transaction = Transaction::new(db.as_ref())?;

            // Insert test state
            reth_provider::insert_canonical_block(
                transaction.deref(),
                SealedBlock::new(case.genesis_block_header.clone().into(), BlockBody::default()),
                None,
            )?;
            case.pre.write_to_db(transaction.deref())?;

            let mut last_block = None;
            for block in case.blocks.iter() {
                last_block = Some(block.write_to_db(transaction.deref())?);
            }

            // Call execution stage
            {
                let mut stage = ExecutionStage::new_with_factory(reth_revm::Factory::new(
                    Arc::new(case.network.clone().into()),
                ));

                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("Could not build tokio RT")
                    .block_on(async {
                        // ignore error
                        let _ = stage
                            .execute(
                                &mut transaction,
                                ExecInput {
                                    previous_stage: last_block.map(|b| {
                                        (StageId::Other("Dummy"), StageCheckpoint::new(b))
                                    }),
                                    checkpoint: None,
                                },
                            )
                            .await;
                    });
            }

            // Validate post state
            match &case.post_state {
                Some(RootOrState::Root(root)) => {
                    // TODO: We should really check the state root here...
                    println!("Post-state root: #{root:?}")
                }
                Some(RootOrState::State(state)) => {
                    for (&address, account) in state.iter() {
                        account.assert_db(address, transaction.deref())?;
                    }
                }
                None => println!("No post-state"),
            }

            transaction.close();
        }
        Ok(())
    }
}

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

    // Skip test where basefee/accesslist/difficulty is present but it shouldn't be supported in
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
        path.file_name() == Some(OsStr::new("CALLBlake2f_MaxRounds.json")) ||
        path.file_name() == Some(OsStr::new("shiftCombinations.json"))
    {
        return true
    }

    // Ignore outdated EOF tests that haven't been updated for Cancun yet.
    let eof_path = Path::new("EIPTests").join("stEOF");
    if path.to_string_lossy().contains(&*eof_path.to_string_lossy()) {
        return true
    }

    false
}
