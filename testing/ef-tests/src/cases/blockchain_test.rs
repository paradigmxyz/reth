//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec, RootOrState},
    Case, Error, Suite,
};
use alloy_rlp::Decodable;
use reth_db::test_utils::create_test_rw_db;
use reth_primitives::{BlockBody, SealedBlock};
use reth_provider::{BlockWriter, ProviderFactory};
use reth_stages::{stages::ExecutionStage, ExecInput, Stage};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

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
            tests: {
                let s = fs::read_to_string(path)
                    .map_err(|error| Error::Io { path: path.into(), error })?;
                serde_json::from_str(&s)
                    .map_err(|error| Error::CouldNotDeserialize { path: path.into(), error })?
            },
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
            let factory = ProviderFactory::new(db.as_ref(), Arc::new(case.network.clone().into()));
            let provider = factory.provider_rw().unwrap();

            // Insert test state
            provider.insert_block(
                SealedBlock::new(case.genesis_block_header.clone().into(), BlockBody::default()),
                None,
                None,
            )?;
            case.pre.write_to_db(provider.tx_ref())?;

            let mut last_block = None;
            for block in case.blocks.iter() {
                let decoded = SealedBlock::decode(&mut block.rlp.as_ref())?;
                last_block = Some(decoded.number);
                provider.insert_block(decoded, None, None)?;
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
                            .execute(&provider, ExecInput { target: last_block, checkpoint: None })
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
                        account.assert_db(address, provider.tx_ref())?;
                    }
                }
                None => println!("No post-state"),
            }

            drop(provider);
        }
        Ok(())
    }
}

/// Returns whether the test at the given path should be skipped.
///
/// Some tests are edge cases that cannot happen on mainnet, while others are skipped for
/// convenience (e.g. they take a long time to run) or are temporarily disabled.
///
/// The reason should be documented in a comment above the file name(s).
pub fn should_skip(path: &Path) -> bool {
    let path_str = path.to_str().expect("Path is not valid UTF-8");
    let name = path.file_name().unwrap().to_str().unwrap();
    matches!(
        name,
        // funky test with `bigint 0x00` value in json :) not possible to happen on mainnet and require
        // custom json parser. https://github.com/ethereum/tests/issues/971
        | "ValueOverflow.json"

        // txbyte is of type 02 and we dont parse tx bytes for this test to fail.
        | "typeTwoBerlin.json"

        // Test checks if nonce overflows. We are handling this correctly but we are not parsing
        // exception in testsuite There are more nonce overflow tests that are in internal
        // call/create, and those tests are passing and are enabled.
        | "CreateTransactionHighNonce.json"

        // Test check if gas price overflows, we handle this correctly but does not match tests specific
        // exception.
        | "HighGasPrice.json"

        // Skip test where basefee/accesslist/difficulty is present but it shouldn't be supported in
        // London/Berlin/TheMerge. https://github.com/ethereum/tests/blob/5b7e1ab3ffaf026d99d20b17bb30f533a2c80c8b/GeneralStateTests/stExample/eip1559.json#L130
        // It is expected to not execute these tests.
        | "accessListExample.json"
        | "basefeeExample.json"
        | "eip1559.json"
        | "mergeTest.json"

        // These tests are passing, but they take a lot of time to execute so we are going to skip them.
        | "loopExp.json"
        | "Call50000_sha256.json"
        | "static_Call50000_sha256.json"
        | "loopMul.json"
        | "CALLBlake2f_MaxRounds.json"
        | "shiftCombinations.json"
    )
    // Ignore outdated EOF tests that haven't been updated for Cancun yet.
    || path_contains(path_str, &["EIPTests", "stEOF"])
}

/// `str::contains` but for a path. Takes into account the OS path separator (`/` or `\`).
fn path_contains(path_str: &str, rhs: &[&str]) -> bool {
    let rhs = rhs.join(std::path::MAIN_SEPARATOR_STR);
    path_str.contains(&rhs)
}
