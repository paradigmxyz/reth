//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec},
    Case, Error, Suite,
};
use alloy_consensus::Block;
use alloy_rlp::{Decodable, Encodable};
use alloy_rpc_types_debug::ExecutionWitness;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_chainspec::ChainSpec;
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_primitives::{BlockBody, RecoveredBlock, SealedBlock, TransactionSigned};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockHashReader, BlockWriter,
    DBProvider, DatabaseProviderFactory, ExecutionOutcome, HashingWriter, HeaderProvider,
    LatestStateProviderRef, OriginalValuesKnown, StateCommitmentProvider, StateProvider,
    StateWriter, StorageLocation,
};
use reth_revm::{database::StateProviderDatabase, witness::ExecutionWitnessRecord, State};
use reth_stateless::validation::stateless_validation;
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
            .try_for_each(run_case)?;

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
    let provider = create_test_provider_factory_with_chain_spec(chain_spec.clone())
        .database_provider_rw()
        .unwrap();

    // Insert the genesis block and the initial test state into the provider
    let genesis_block = SealedBlock::<reth_primitives::Block>::from_sealed_parts(
        case.genesis_block_header.clone().into(),
        BlockBody::default(),
    )
    .try_recover()
    .unwrap();

    provider.insert_block(genesis_block.clone(), StorageLocation::Database)?;
    case.pre.write_to_db(provider.tx_ref())?;

    // Decode and insert blocks, creating a chain of blocks for the test case.
    let mut blocks_with_genesis = Vec::with_capacity(case.blocks.len() + 1);
    blocks_with_genesis.push(genesis_block);
    for block in &case.blocks {
        let decoded = SealedBlock::<reth_primitives::Block>::decode(&mut block.rlp.as_ref())?;
        let recovered_block = decoded.clone().try_recover().unwrap();

        provider.insert_block(recovered_block.clone(), StorageLocation::Database)?;

        blocks_with_genesis.push(recovered_block);
    }
    let last_block = blocks_with_genesis.last().cloned();

    match execute_blocks(&provider, &blocks_with_genesis, chain_spec.clone(), |maybe_block| {
        match maybe_block {
            Some(block) => provider.history_by_block_hash(block.parent_hash).unwrap(),
            None => Box::new(LatestStateProviderRef::new(&provider)),
        }
    }) {
        Err(Error::BlockExecutionFailed) => {
            // If block execution failed, then we don't generate a stateless witness, but we still
            // do the post state checks
        }
        Ok(program_inputs) => {
            for (block, execution_witness) in program_inputs {
                stateless_validation(block, execution_witness, chain_spec.clone())
                    .expect("stateless validation failed");
            }
        }
        Err(err) => return Err(err),
    };

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
}

fn execute_blocks<
    Provider: DBProvider
        + StateWriter<Receipt = reth_primitives::Receipt>
        + BlockHashReader
        + HeaderProvider
        + StateCommitmentProvider,
    F: FnMut(Option<&RecoveredBlock<Block<TransactionSigned>>>) -> SP,
    SP: StateProvider,
>(
    provider: &Provider,
    blocks_with_genesis: &[RecoveredBlock<Block<TransactionSigned>>],
    chain_spec: Arc<ChainSpec>,
    mut create_state_provider: F,
) -> Result<Vec<(RecoveredBlock<Block<TransactionSigned>>, ExecutionWitness)>, Error> {
    let executor_provider = EthExecutorProvider::ethereum(chain_spec);

    // First execute all of the blocks
    // TODO: We have two loops because if we use provider.latest() the merkle tree path
    // TODO: is not correct
    for block in blocks_with_genesis.iter().skip(1) {
        let state_provider = create_state_provider(None);
        let state_db = StateProviderDatabase(&state_provider);
        let block_executor = executor_provider.executor(state_db);
        let output = block_executor.execute(block).map_err(|_| Error::BlockExecutionFailed)?;
        provider.write_state(
            &ExecutionOutcome::single(block.number, output),
            OriginalValuesKnown::Yes,
            StorageLocation::Database,
        )?;
    }

    let mut program_inputs = Vec::new();

    for block in blocks_with_genesis.iter().skip(1) {
        let block_number = block.number;
        let state_provider = create_state_provider(Some(block));
        let state_db = StateProviderDatabase(&state_provider);
        let block_executor = executor_provider.executor(state_db);

        let mut witness_record = ExecutionWitnessRecord::default();

        block_executor
            .execute_with_state_closure(&(*block).clone(), |statedb: &State<_>| {
                witness_record.record_executed_state(statedb);
            })
            .map_err(|_| Error::BlockExecutionFailed)?;

        // TODO: Most of this code is copy-pasted from debug_executionWitness
        let ExecutionWitnessRecord { hashed_state, codes, keys, lowest_block_number } =
            witness_record;
        let state = state_provider.witness(Default::default(), hashed_state)?;
        let mut exec_witness = ExecutionWitness { state, codes, keys, headers: Default::default() };

        let smallest = match lowest_block_number {
            Some(smallest) => smallest,
            None => {
                // Return only the parent header, if there were no calls to the
                // BLOCKHASH opcode.
                block_number.saturating_sub(1)
            }
        };

        let range = smallest..block_number;

        exec_witness.headers = provider
            .headers_range(range)?
            // .map_err(EthApiError::from)?
            .into_iter()
            .map(|header| {
                let mut serialized_header = Vec::new();
                header.encode(&mut serialized_header);
                serialized_header.into()
            })
            .collect();

        program_inputs.push((block.clone(), exec_witness));
    }

    Ok(program_inputs)
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
