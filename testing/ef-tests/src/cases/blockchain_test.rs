//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    models::{BlockchainTest, ForkSpec},
    Case, Error, Suite,
};
use alloy_consensus::Block;
use alloy_rlp::Decodable;
use alloy_rpc_types_debug::ExecutionWitness;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_chainspec::ChainSpec;
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_primitives::{BlockBody, RecoveredBlock, SealedBlock, TransactionSigned};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockHashReader, BlockWriter,
    DBProvider, DatabaseProviderFactory, ExecutionOutcome, HashingWriter, LatestStateProviderRef,
    OriginalValuesKnown, StateCommitmentProvider, StateProvider, StateWriter, StorageLocation,
};
use reth_revm::{database::StateProviderDatabase, witness::ExecutionWitnessRecord, State};
use reth_stateless::validation::{stateless_validation, Input};
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
        Ok(execution_witnesses) => {
            let stateless_inputs =
                compute_stateless_input(execution_witnesses, &blocks_with_genesis, chain_spec);

            for stateless_input in stateless_inputs {
                stateless_validation(
                    stateless_input.current_block,
                    stateless_input.execution_witness,
                    stateless_input.ancestor_headers,
                    stateless_input.chain_spec,
                )
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
        + StateCommitmentProvider,
    F: FnMut(Option<&RecoveredBlock<Block<TransactionSigned>>>) -> SP,
    SP: StateProvider,
>(
    provider: &Provider,
    blocks_with_genesis: &[RecoveredBlock<Block<TransactionSigned>>],
    chain_spec: Arc<ChainSpec>,
    mut create_state_provider: F,
) -> Result<Vec<ExecutionWitness>, Error> {
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

    let mut exec_witnesses = Vec::new();

    for block in blocks_with_genesis.iter().skip(1) {
        let state_provider = create_state_provider(Some(block));
        let state_db = StateProviderDatabase(&state_provider);
        let block_executor = executor_provider.executor(state_db);

        let mut witness_record = ExecutionWitnessRecord::default();

        block_executor
            .execute_with_state_closure(&(*block).clone(), |statedb: &State<_>| {
                witness_record.record_executed_state(statedb);
            })
            .map_err(|_| Error::BlockExecutionFailed)?;

        let ExecutionWitnessRecord { hashed_state, codes, keys } = witness_record;
        let state = state_provider.witness(Default::default(), hashed_state)?;
        let exec_witness = ExecutionWitness { state, codes, keys };
        exec_witnesses.push(exec_witness);
    }

    Ok(exec_witnesses)
}

/// Selects a contiguous range of items from a slice, ending just before the specified `end_index`.
///
/// # Arguments
///
/// * `slice` - The source slice to select items from
/// * `end_index` - The exclusive end index for selection (items up to but not including this index)
/// * `num_items_to_select` - The number of items to select
fn select_items_before_index<T: Clone>(
    slice: &[T],
    end_index: usize,
    num_items_to_select: usize,
) -> Option<Vec<T>> {
    if end_index == 0 || end_index > slice.len() || num_items_to_select == 0 {
        return None;
    }

    let end_pos = end_index - 1;
    let start_pos = end_pos.saturating_sub(num_items_to_select - 1);
    slice.iter().skip(start_pos).take(end_pos - start_pos + 1).cloned().collect::<Vec<_>>().into()
}

fn compute_stateless_input(
    execution_witnesses: Vec<ExecutionWitness>,
    blocks_with_genesis: &[RecoveredBlock<Block<TransactionSigned>>],
    chain_spec: Arc<ChainSpec>,
) -> Vec<Input> {
    let mut guest_inputs = Vec::new();

    assert_eq!(blocks_with_genesis.len() - 1, execution_witnesses.len());

    for (current_block, execution_witness) in
        // Skip the genesis block, it has no ancestors and no execution
        blocks_with_genesis.iter().skip(1).zip(execution_witnesses)
    {
        let ancestor_blocks =
            select_items_before_index(blocks_with_genesis, current_block.number as usize, 256)
                .expect("could not fetch ancestors");
        let ancestor_headers =
            ancestor_blocks.into_iter().map(|block| block.clone_header()).collect();
        guest_inputs.push(Input {
            current_block: current_block.clone(),
            execution_witness,
            ancestor_headers,
            chain_spec: chain_spec.clone(),
        });
    }

    guest_inputs
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_selection() {
        let items = vec![0, 1, 2, 3, 4];

        // Select 3 items ending before index 4
        assert_eq!(select_items_before_index(&items, 4, 3), Some(vec![1, 2, 3]));

        // Select 2 items ending before index 3
        assert_eq!(select_items_before_index(&items, 3, 2), Some(vec![1, 2]));

        // Select 1 item ending before index 2
        assert_eq!(select_items_before_index(&items, 2, 1), Some(vec![1]));
    }

    #[test]
    fn test_edge_cases() {
        let items = vec![0, 1, 2, 3, 4];

        // Select all items before the last
        assert_eq!(select_items_before_index(&items, 4, 4), Some(vec![0, 1, 2, 3]));

        // Select item before index 1
        assert_eq!(select_items_before_index(&items, 1, 1), Some(vec![0]));

        // Index is at the end of the slice
        assert_eq!(select_items_before_index(&items, 5, 2), Some(vec![3, 4]));
    }

    #[test]
    fn test_invalid_inputs() {
        let items = vec![0, 1, 2, 3, 4];

        // end_index is 0
        assert_eq!(select_items_before_index(&items, 0, 1), None);

        // end_index beyond slice bounds
        assert_eq!(select_items_before_index(&items, 6, 3), None);

        // num_items_to_select is 0
        assert_eq!(select_items_before_index(&items, 3, 0), None);

        // Empty slice
        let empty: Vec<i32> = vec![];
        assert_eq!(select_items_before_index(&empty, 1, 1), None);
    }
}
