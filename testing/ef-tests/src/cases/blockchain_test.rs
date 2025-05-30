//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec},
    Case, Error, Suite,
};
use alloy_rlp::{Decodable, Encodable};
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_chainspec::ChainSpec;
use reth_consensus::{Consensus, HeaderValidator};
use reth_db_common::init::{insert_genesis_hashes, insert_genesis_history, insert_genesis_state};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_ethereum_primitives::Block;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_evm_ethereum::{execute::EthExecutorProvider, EthEvmConfig};
use reth_primitives_traits::{RecoveredBlock, SealedBlock};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockWriter, DatabaseProviderFactory,
    ExecutionOutcome, HeaderProvider, HistoryWriter, OriginalValuesKnown, StateProofProvider,
    StateWriter, StorageLocation,
};
use reth_revm::{database::StateProviderDatabase, witness::ExecutionWitnessRecord, State};
use reth_stateless::{validation::stateless_validation, ExecutionWitness};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
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

impl BlockchainTestCase {
    /// Returns `true` if the fork is not supported.
    const fn excluded_fork(network: ForkSpec) -> bool {
        matches!(
            network,
            ForkSpec::ByzantiumToConstantinopleAt5 |
                ForkSpec::Constantinople |
                ForkSpec::ConstantinopleFix |
                ForkSpec::MergeEOF |
                ForkSpec::MergeMeterInitCode |
                ForkSpec::MergePush0
        )
    }

    /// Checks if the test case is a particular test called `UncleFromSideChain`
    ///
    /// This fixture fails as expected, however it fails at the wrong block number.
    /// Given we no longer have uncle blocks, this test case was pulled out such
    /// that we ensure it still fails as expected, however we do not check the block number.
    #[inline]
    fn is_uncle_sidechain_case(name: &str) -> bool {
        name.contains("UncleFromSideChain")
    }

    /// If the test expects an exception, return the block number
    /// at which it must occur together with the original message.
    ///
    /// Note: There is a +1 here because the genesis block is not included
    /// in the set of blocks, so the first block is actually block number 1
    /// and not block number 0.
    #[inline]
    fn expected_failure(case: &BlockchainTest) -> Option<(u64, String)> {
        case.blocks.iter().enumerate().find_map(|(idx, blk)| {
            blk.expect_exception.as_ref().map(|msg| ((idx + 1) as u64, msg.clone()))
        })
    }

    /// Execute a single `BlockchainTest`, validating the outcome against the
    /// expectations encoded in the JSON file.
    fn run_single_case(name: &str, case: &BlockchainTest) -> Result<(), Error> {
        let expectation = Self::expected_failure(case);
        match run_case(case) {
            // All blocks executed successfully.
            Ok(()) => {
                // Check if the test case specifies that it should have failed
                if let Some((block, msg)) = expectation {
                    Err(Error::Assertion(format!(
                        "Test case: {name}\nExpected failure at block {block} - {msg}, but all blocks succeeded",
                    )))
                } else {
                    Ok(())
                }
            }

            // A block processing failure occurred.
            Err(Error::BlockProcessingFailed { block_number }) => match expectation {
                // It happened on exactly the block we were told to fail on
                Some((expected, _)) if block_number == expected => Ok(()),

                // Uncle side‑chain edge case, we accept as long as it failed.
                // But we don't check the exact block number.
                _ if Self::is_uncle_sidechain_case(name) => Ok(()),

                // Expected failure, but block number does not match
                Some((expected, _)) => Err(Error::Assertion(format!(
                    "Test case: {name}\nExpected failure at block {expected}\nGot failure at block {block_number}",
                ))),

                // No failure expected at all - bubble up original error.
                None => Err(Error::BlockProcessingFailed { block_number }),
            },

            // Non‑processing error – forward as‑is.
            //
            // This should only happen if we get an unexpected error from processing the block.
            // Since it is unexpected, we treat it as a test failure.
            //
            // One reason for this happening is when one forgets to wrap the error from `run_case`
            // so that it produces a `Error::BlockProcessingFailed`
            Err(other) => Err(other),
        }
    }
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
            .iter()
            .filter(|(_, case)| !Self::excluded_fork(case.network))
            .par_bridge()
            .try_for_each(|(name, case)| Self::run_single_case(name, case))?;

        Ok(())
    }
}

/// Executes a single `BlockchainTest`, returning an error if the blockchain state
/// does not match the expected outcome after all blocks are executed.
///
/// A `BlockchainTest` represents a self-contained scenario:
/// - It initializes a fresh blockchain state.
/// - It sequentially decodes, executes, and inserts a predefined set of blocks.
/// - It then verifies that the resulting blockchain state (post-state) matches the expected
///   outcome.
///
/// Returns:
/// - `Ok(())` if all blocks execute successfully and the final state is correct.
/// - `Err(Error)` if any block fails to execute correctly, or if the post-state validation fails.
fn run_case(case: &BlockchainTest) -> Result<(), Error> {
    // Create a new test database and initialize a provider for the test case.
    let chain_spec: Arc<ChainSpec> = Arc::new(case.network.into());
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let provider = factory.database_provider_rw().unwrap();

    // Insert initial test state into the provider.
    let genesis_block = SealedBlock::<Block>::from_sealed_parts(
        case.genesis_block_header.clone().into(),
        Default::default(),
    )
    .try_recover()
    .unwrap();

    provider
        .insert_block(genesis_block.clone(), StorageLocation::Database)
        .map_err(|_| Error::BlockProcessingFailed { block_number: 0 })?;

    let genesis_state = case.pre.clone().into_genesis_state();
    insert_genesis_state(&provider, genesis_state.iter())
        .map_err(|_| Error::BlockProcessingFailed { block_number: 0 })?;
    insert_genesis_hashes(&provider, genesis_state.iter())
        .map_err(|_| Error::BlockProcessingFailed { block_number: 0 })?;
    insert_genesis_history(&provider, genesis_state.iter())
        .map_err(|_| Error::BlockProcessingFailed { block_number: 0 })?;

    // Decode blocks
    let blocks = decode_blocks(&case.blocks)?;

    let executor_provider = EthExecutorProvider::ethereum(chain_spec.clone());
    let mut parent = genesis_block;
    let mut program_inputs = Vec::new();

    for (block_index, block) in blocks.iter().enumerate() {
        // Note: same as the comment on `decode_blocks` as to why we cannot use block.number
        let block_number = (block_index + 1) as u64;

        // Insert the block into the database
        provider
            .insert_block(block.clone(), StorageLocation::Database)
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        // Consensus checks before block execution
        pre_execution_checks(chain_spec.clone(), &parent, block)
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        let mut witness_record = ExecutionWitnessRecord::default();

        // Execute the block
        let state_provider = provider.latest();
        let state_db = StateProviderDatabase(&state_provider);
        let executor = executor_provider.batch_executor(state_db);

        let output = executor
            .execute_with_state_closure(&(*block).clone(), |statedb: &State<_>| {
                witness_record.record_executed_state(statedb);
            })
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        // Consensus checks after block execution
        validate_block_post_execution(block, &chain_spec, &output.receipts, &output.requests)
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        // Generate the stateless witness
        // TODO: Most of this code is copy-pasted from debug_executionWitness
        let ExecutionWitnessRecord { hashed_state, codes, keys, lowest_block_number } =
            witness_record;
        let state = state_provider.witness(Default::default(), hashed_state)?;
        let mut exec_witness = ExecutionWitness { state, codes, keys, headers: Default::default() };

        let smallest = lowest_block_number.unwrap_or_else(|| {
            // Return only the parent header, if there were no calls to the
            // BLOCKHASH opcode.
            block_number.saturating_sub(1)
        });

        let range = smallest..block_number;

        exec_witness.headers = provider
            .headers_range(range)?
            .into_iter()
            .map(|header| {
                let mut serialized_header = Vec::new();
                header.encode(&mut serialized_header);
                serialized_header.into()
            })
            .collect();

        program_inputs.push((block.clone(), exec_witness));

        // Compute and check the post state root
        let hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state());
        let (computed_state_root, _) =
            StateRoot::overlay_root_with_updates(provider.tx_ref(), hashed_state.clone())
                .map_err(|_| Error::BlockProcessingFailed { block_number })?;
        if computed_state_root != block.state_root {
            return Err(Error::BlockProcessingFailed { block_number })
        }

        // Commit the post state/state diff to the database
        provider
            .write_state(
                &ExecutionOutcome::single(block.number, output),
                OriginalValuesKnown::Yes,
                StorageLocation::Database,
            )
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        provider
            .write_hashed_state(&hashed_state.into_sorted())
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;
        provider
            .update_history_indices(block.number..=block.number)
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        // Since there were no errors, update the parent block
        parent = block.clone()
    }

    // Validate the post-state for the test case.
    //
    // If we get here then it means that the post-state root checks
    // made after we execute each block was successful.
    //
    // If an error occurs here, then it is:
    // - Either an issue with the test setup
    // - Possibly an error in the test case where the post-state root in the last block does not
    //   match the post-state values.
    let expected_post_state = case.post_state.as_ref().ok_or(Error::MissingPostState)?;
    for (&address, account) in expected_post_state {
        account.assert_db(address, provider.tx_ref())?;
    }

    // Now validate using the stateless client if everything else passes
    for (block, execution_witness) in program_inputs {
        stateless_validation(
            block.into_block(),
            execution_witness,
            chain_spec.clone(),
            EthEvmConfig::new(chain_spec.clone()),
        )
        .expect("stateless validation failed");
    }

    Ok(())
}

fn decode_blocks(
    test_case_blocks: &[crate::models::Block],
) -> Result<Vec<RecoveredBlock<Block>>, Error> {
    let mut blocks = Vec::with_capacity(test_case_blocks.len());
    for (block_index, block) in test_case_blocks.iter().enumerate() {
        // The blocks do not include the genesis block which is why we have the plus one.
        // We also cannot use block.number because for invalid blocks, this may be incorrect.
        let block_number = (block_index + 1) as u64;

        let decoded = SealedBlock::<Block>::decode(&mut block.rlp.as_ref())
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        let recovered_block = decoded
            .clone()
            .try_recover()
            .map_err(|_| Error::BlockProcessingFailed { block_number })?;

        blocks.push(recovered_block);
    }

    Ok(blocks)
}

fn pre_execution_checks(
    chain_spec: Arc<ChainSpec>,
    parent: &RecoveredBlock<Block>,
    block: &RecoveredBlock<Block>,
) -> Result<(), Error> {
    let consensus: EthBeaconConsensus<ChainSpec> = EthBeaconConsensus::new(chain_spec);

    let sealed_header = block.sealed_header();

    <EthBeaconConsensus<ChainSpec> as Consensus<Block>>::validate_body_against_header(
        &consensus,
        block.body(),
        sealed_header,
    )?;
    consensus.validate_header_against_parent(sealed_header, parent.sealed_header())?;
    consensus.validate_header(sealed_header)?;
    consensus.validate_block_pre_execution(block)?;

    Ok(())
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
