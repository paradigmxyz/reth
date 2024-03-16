//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec},
    Case, Error, Suite,
};
use alloy_rlp::Decodable;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{BlockBody, SealedBlock, StaticFileSegment};
use reth_provider::{providers::StaticFileWriter, HashingWriter, ProviderFactory};
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

    /// Runs the test cases for the Ethereum Forks test suite.
    ///
    /// # Errors
    /// Returns an error if the test is flagged for skipping or encounters issues during execution.
    fn run(&self) -> Result<(), Error> {
        // If the test is marked for skipping, return a Skipped error immediately.
        if self.skip {
            return Err(Error::Skipped)
        }

        // Iterate through test cases, filtering by the network type to exclude specific forks.
        self.tests
            .values()
            .filter(|case| {
                !matches!(
                    case.network,
                    ForkSpec::ByzantiumToConstantinopleAt5 |
                        ForkSpec::Constantinople |
                        ForkSpec::ConstantinopleFix |
                        ForkSpec::MergeEOF |
                        ForkSpec::MergeMeterInitCode |
                        ForkSpec::MergePush0 |
                        ForkSpec::Unknown
                )
            })
            .par_bridge()
            .try_for_each(|case| {
                // Create a new test database and initialize a provider for the test case.
                let db = create_test_rw_db();
                let (_static_files_dir, static_files_dir_path) = create_test_static_files_dir();
                let provider = ProviderFactory::new(
                    db.as_ref(),
                    Arc::new(case.network.clone().into()),
                    static_files_dir_path,
                )?
                .provider_rw()
                .unwrap();

                // Insert initial test state into the provider.
                provider
                    .insert_historical_block(
                        SealedBlock::new(
                            case.genesis_block_header.clone().into(),
                            BlockBody::default(),
                        )
                        .try_seal_with_senders()
                        .unwrap(),
                        None,
                    )
                    .map_err(|err| Error::RethError(err.into()))?;
                case.pre.write_to_db(provider.tx_ref())?;

                // Initialize receipts static file with genesis
                {
                    let mut receipts_writer = provider
                        .static_file_provider()
                        .latest_writer(StaticFileSegment::Receipts)
                        .unwrap();
                    receipts_writer.increment_block(StaticFileSegment::Receipts, 0).unwrap();
                    receipts_writer.commit_without_sync_all().unwrap();
                }

                // Decode and insert blocks, creating a chain of blocks for the test case.
                let last_block = case.blocks.iter().try_fold(None, |_, block| {
                    let decoded = SealedBlock::decode(&mut block.rlp.as_ref())?;
                    provider
                        .insert_historical_block(
                            decoded.clone().try_seal_with_senders().unwrap(),
                            None,
                        )
                        .map_err(|err| Error::RethError(err.into()))?;
                    Ok::<Option<SealedBlock>, Error>(Some(decoded))
                })?;
                provider
                    .static_file_provider()
                    .latest_writer(StaticFileSegment::Headers)
                    .unwrap()
                    .commit_without_sync_all()
                    .unwrap();

                // Execute the execution stage using the EVM processor factory for the test case
                // network.
                let _ = ExecutionStage::new_with_factory(reth_revm::EvmProcessorFactory::new(
                    Arc::new(case.network.clone().into()),
                    EthEvmConfig::default(),
                ))
                .execute(
                    &provider,
                    ExecInput { target: last_block.as_ref().map(|b| b.number), checkpoint: None },
                );

                // Validate the post-state for the test case.
                match (&case.post_state, &case.post_state_hash) {
                    (Some(state), None) => {
                        // Validate accounts in the state against the provider's database.
                        for (&address, account) in state.iter() {
                            account.assert_db(address, provider.tx_ref())?;
                        }
                    }
                    (None, Some(expected_state_root)) => {
                        // Insert state hashes into the provider based on the expected state root.
                        let last_block = last_block.unwrap_or_default();
                        provider
                            .insert_hashes(
                                0..=last_block.number,
                                last_block.hash(),
                                *expected_state_root,
                            )
                            .map_err(|err| Error::RethError(err.into()))?;
                    }
                    _ => return Err(Error::MissingPostState),
                }

                // Drop the provider without committing to the database.
                drop(provider);
                Ok(())
            })?;

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
        | "ValueOverflowParis.json"

        // txbyte is of type 02 and we dont parse tx bytes for this test to fail.
        | "typeTwoBerlin.json"

        // Test checks if nonce overflows. We are handling this correctly but we are not parsing
        // exception in testsuite There are more nonce overflow tests that are in internal
        // call/create, and those tests are passing and are enabled.
        | "CreateTransactionHighNonce.json"

        // Test check if gas price overflows, we handle this correctly but does not match tests specific
        // exception.
        | "HighGasPrice.json"
        | "HighGasPriceParis.json"

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
