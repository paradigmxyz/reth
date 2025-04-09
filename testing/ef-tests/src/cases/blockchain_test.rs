//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec},
    Case, Error, Suite,
};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::U64;
use alloy_rlp::Decodable;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_chainspec::ChainSpec;
use reth_db_common::init::init_genesis;
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_primitives::{BlockBody, SealedBlock, StaticFileSegment};
use reth_provider::{
    providers::StaticFileWriter, test_utils::create_test_provider_factory_with_chain_spec,
    DatabaseProviderFactory, ExecutionOutcome, HashingWriter, OriginalValuesKnown, StateWriter,
    StaticFileProviderFactory, StorageLocation,
};
use reth_revm::database::StateProviderDatabase;
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

/// A handler for the blockchain test suite.
#[derive(Debug)]
pub struct BlockchainTests {
    suite: String,
}

impl BlockchainTests {
    /// Create a new handler for a subset of the blockchain test suite.
    pub const fn new(suite: String) -> Self {
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
        Ok(Self {
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
                let mut chain_spec: ChainSpec = case.network.into();

                let gen_accounts: BTreeMap<_, _> = case
                    .pre
                    .iter()
                    .map(|(addr, acc)| (addr.clone(), GenesisAccount::from(acc.clone())))
                    .collect();

                let genesis_block = SealedBlock::<reth_primitives::Block>::from_sealed_parts(
                    case.genesis_block_header.clone().into(),
                    BlockBody::default(),
                )
                .try_recover()
                .unwrap();

                // Fix-up chain_spec -- TODO: This is hacky, clean up
                chain_spec.genesis = Genesis {
                    config: chain_spec.genesis.config,
                    nonce: U64::from_be_bytes(genesis_block.nonce.0).to::<u64>(),
                    timestamp: genesis_block.timestamp,
                    extra_data: genesis_block.extra_data.clone(),
                    gas_limit: genesis_block.gas_limit.clone(),
                    difficulty: genesis_block.clone().difficulty,
                    mix_hash: genesis_block.clone().mix_hash,
                    coinbase: genesis_block.clone_header().beneficiary,
                    alloc: gen_accounts.clone(),
                    base_fee_per_gas: genesis_block.base_fee_per_gas.map(|x| x as u128),
                    excess_blob_gas: genesis_block.excess_blob_gas,
                    blob_gas_used: genesis_block.blob_gas_used,
                    number: Some(genesis_block.number),
                };
                chain_spec.genesis_header = genesis_block.clone_sealed_header();

                let chain_spec_genesis_hash = chain_spec.genesis_hash();

                let chain_spec = Arc::new(chain_spec);

                let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());

                assert_eq!(
                    chain_spec_genesis_hash,
                    genesis_block.hash_slow(),
                    "shouldn't these hashes be the same?"
                );

                init_genesis(&factory).unwrap();

                let provider = factory.database_provider_rw().unwrap();

                // Decode and insert blocks, creating a chain of blocks for the test case.
                let mut blocks = Vec::new();
                let last_block = case.blocks.iter().try_fold(None, |_, block| {
                    let decoded =
                        SealedBlock::<reth_primitives::Block>::decode(&mut block.rlp.as_ref())?;
                    let recovered = decoded.clone().try_recover().unwrap();
                    provider.insert_historical_block(recovered.clone())?;

                    blocks.push(recovered);

                    Ok::<Option<SealedBlock>, Error>(Some(decoded))
                })?;
                provider
                    .static_file_provider()
                    .latest_writer(StaticFileSegment::Headers)
                    .unwrap()
                    .commit_without_sync_all()
                    .unwrap();

                let executor_provider = EthExecutorProvider::ethereum(chain_spec);
                for block in blocks {
                    let state_provider = provider.history_by_block_hash(block.parent_hash)?;
                    let state_db = StateProviderDatabase(state_provider);
                    let executor = executor_provider.executor(state_db);

                    // Execute all blocks in a batch
                    if let Ok(output) = executor.execute(&block) {
                        provider.write_state(
                            &ExecutionOutcome::single(block.number, output),
                            OriginalValuesKnown::Yes,
                            StorageLocation::StaticFiles,
                        )?;
                    }
                }

                // Validate the post-state for the test case.
                match (&case.post_state, &case.post_state_hash) {
                    (Some(state), None) => {
                        // Validate accounts in the state against the provider's database.
                        for (&address, account) in state {
                            account.assert_db(address, provider.tx_ref())?;
                        }
                    }
                    (None, Some(expected_state_root)) => {
                        // Insert state hashes into the provider based on the expected state root.
                        let last_block = last_block.unwrap_or_default();
                        provider.insert_hashes(
                            0..=last_block.number,
                            last_block.hash(),
                            *expected_state_root,
                        )?;
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

        // txbyte is of type 02 and we don't parse tx bytes for this test to fail.
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

        // Skipped by revm as well: <https://github.com/bluealloy/revm/blob/be92e1db21f1c47b34c5a58cfbf019f6b97d7e4b/bins/revme/src/cmd/statetest/runner.rs#L115-L125>
        | "RevertInCreateInInit_Paris.json"
        | "RevertInCreateInInit.json"
        | "dynamicAccountOverwriteEmpty.json"
        | "dynamicAccountOverwriteEmpty_Paris.json"
        | "RevertInCreateInInitCreate2Paris.json"
        | "create2collisionStorage.json"
        | "RevertInCreateInInitCreate2.json"
        | "create2collisionStorageParis.json"
        | "InitCollision.json"
        | "InitCollisionParis.json"
    )
    // Ignore outdated EOF tests that haven't been updated for Cancun yet.
    || path_contains(path_str, &["EIPTests", "stEOF"])
}

/// `str::contains` but for a path. Takes into account the OS path separator (`/` or `\`).
fn path_contains(path_str: &str, rhs: &[&str]) -> bool {
    let rhs = rhs.join(std::path::MAIN_SEPARATOR_STR);
    path_str.contains(&rhs)
}
