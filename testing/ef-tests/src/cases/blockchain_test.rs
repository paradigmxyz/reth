//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    case::load_json,
    models::{BlockchainTest, ForkSpec},
    result::{FixtureResult, Rejection},
    Case, Error, Suite,
};
use alloy_eip7928::bal::Bal;
use alloy_eips::{
    eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
};
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_consensus::{Consensus, ConsensusError, HeaderValidator};
use reth_db_common::init::{insert_genesis_hashes, insert_genesis_history, insert_genesis_state};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_ethereum_primitives::Block;
use reth_evm::{
    database::StateProviderDatabase,
    execute::{BlockExecutionOutput, Executor},
    ConfigureEvm,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{GotExpected, ParallelBridgeBuffered, RecoveredBlock, SealedBlock};
use reth_provider::{
    providers::{RocksDBBuilder, RocksDBProvider, StaticFileProviderBuilder},
    test_utils::MockNodeTypesWithDB,
    BlockWriter, DatabaseProviderFactory, ExecutionOutcome, HashedPostStateProvider, HistoryWriter,
    OriginalValuesKnown, ProviderFactory, StateProvider, StateWriteConfig, StateWriter,
    StaticFileProviderFactory, StaticFileSegment, StaticFileWriter, StorageSettingsCache,
    TrieWriter,
};
use reth_tasks::Runtime;
use reth_trie::StateRoot;
use reth_trie_db::DatabaseStateRoot;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

/// A handler for the blockchain test suite.
#[derive(Debug)]
pub struct BlockchainTests {
    suite_path: PathBuf,
}

impl BlockchainTests {
    /// Create a new suite for tests with blockchain tests format.
    pub const fn new(suite_path: PathBuf) -> Self {
        Self { suite_path }
    }

    /// Runs every fixture in the JSON `files`, one file at a time on each of `workers` threads,
    /// and passes one result per fixture to `on_result` as it completes.
    ///
    /// Unlike [`Suite::run`], this reports a file that fails to load as a failed result instead
    /// of panicking.
    ///
    /// The files are not run on the rayon pool: a rayon thread waiting on block execution's own
    /// parallel work would start other files meanwhile, with no bound on how many are open.
    pub fn run_fixtures(
        files: Vec<PathBuf>,
        workers: usize,
        on_result: &(dyn Fn(FixtureResult) + Sync),
    ) {
        let files = Mutex::new(files.into_iter());
        // A closure, so the queue's lock is released before the file runs: a guard taken in the
        // `while let` condition would be held until the end of the loop body.
        let next_file = || files.lock().unwrap().next();
        std::thread::scope(|scope| {
            for _ in 0..workers.max(1) {
                scope.spawn(|| {
                    while let Some(path) = next_file() {
                        Self::run_file(&path, on_result);
                    }
                });
            }
        });
    }

    /// Runs every fixture in the JSON file at `path` and passes one result per fixture to
    /// `on_result`.
    fn run_file(path: &Path, on_result: &dyn Fn(FixtureResult)) {
        let case = match BlockchainTestCase::load(path) {
            Ok(case) => case,
            Err(err) => return on_result(FixtureResult::load_failed(path, err)),
        };
        for (name, test) in case.tests {
            if BlockchainTestCase::excluded_fork(test.network) {
                continue
            }
            let fork = format!("{:?}", test.network);
            if case.skip {
                on_result(FixtureResult::new(name, fork, Err(Error::Skipped)));
                continue
            }
            let mut last_block_hash = test.genesis_block_header.hash;
            let mut rejections = Vec::new();
            let result = BlockchainTestCase::run_single_case_with(
                &name,
                &test,
                &mut last_block_hash,
                &mut rejections,
            )
            .and_then(|()| check_last_block_hash(&test, last_block_hash));
            on_result(
                FixtureResult::new(name, fork, result)
                    .with_last_block_hash(last_block_hash)
                    .with_rejections(rejections),
            );
        }
    }
}

impl Suite for BlockchainTests {
    type Case = BlockchainTestCase;

    fn suite_path(&self) -> &Path {
        &self.suite_path
    }
}

/// An Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockchainTestCase {
    /// The tests within this test case.
    pub tests: BTreeMap<String, BlockchainTest>,
    /// Whether to skip this test case.
    pub skip: bool,
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

    /// Returns `true` if the case runs Prague blocks without the EIP-7002 and EIP-7251 request
    /// contracts, which makes them invalid. Legacy fixtures predate those predeploys, and their
    /// state roots rule out adding them.
    fn lacks_request_contracts(case: &BlockchainTest) -> bool {
        case.network.to_chain_spec().is_prague_active_at_timestamp(u64::MAX) &&
            [WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS]
                .into_iter()
                .any(|address| {
                    case.pre.get(&address).is_none_or(|account| account.code.is_empty())
                })
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
    pub fn run_single_case(name: &str, case: &BlockchainTest) -> Result<(), Error> {
        let mut last_block_hash = B256::ZERO;
        Self::run_single_case_with(name, case, &mut last_block_hash, &mut Vec::new())
    }

    /// Like [`Self::run_single_case`], but `last_block_hash` is set to the hash of the last block
    /// that was imported, or of genesis if none was, and the block that was rejected, if any, is
    /// added to `rejections` with reth's error.
    pub fn run_single_case_with(
        name: &str,
        case: &BlockchainTest,
        last_block_hash: &mut B256,
        rejections: &mut Vec<Rejection>,
    ) -> Result<(), Error> {
        let expectation = Self::expected_failure(case);
        let result = run_case(case, last_block_hash);
        // Block number 0 is the genesis setup, not a fixture block.
        if let Err(Error::BlockProcessingFailed { block_number, err }) = &result &&
            *block_number > 0
        {
            rejections.push(rejection(case, *block_number, err.to_string()));
        }
        match result {
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
            Err(Error::BlockProcessingFailed { block_number, err }) => {
                match expectation {
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
                    None => Err(Error::BlockProcessingFailed { block_number, err }),
                }
            }

            // Non‑processing error – forward as‑is.
            //
            // This should only happen if we get an unexpected error from processing the block.
            // Since it is unexpected, we treat it as a test failure.
            //
            // One reason for this happening is when one forgets to wrap the error from `run_case`
            // so that it produces an `Error::BlockProcessingFailed`
            Err(other) => Err(other),
        }
    }
}

impl Case for BlockchainTestCase {
    fn load(path: &Path) -> Result<Self, Error> {
        Ok(Self { tests: load_json(path)?, skip: should_skip(path) })
    }

    /// Runs the test cases for the Ethereum Forks test suite.
    ///
    /// # Errors
    /// Returns an error if the test is flagged for skipping or encounters issues during execution.
    fn run(self) -> Result<(), Error> {
        // If the test is marked for skipping, return a Skipped error immediately.
        if self.skip {
            return Err(Error::Skipped);
        }

        // Iterate through test cases, filtering by the network type to exclude specific forks.
        self.tests
            .into_iter()
            .filter(|(_, case)| {
                !Self::excluded_fork(case.network) && !Self::lacks_request_contracts(case)
            })
            .par_bridge_buffered()
            .with_min_len(64)
            .try_for_each(|(name, case)| Self::run_single_case(&name, &case).map(|_| ()))
    }
}

/// Executes a single `BlockchainTest` returning an error as soon as any block has a consensus
/// validation failure.
///
/// A `BlockchainTest` represents a self-contained scenario:
/// - It initializes a fresh blockchain state.
/// - It sequentially decodes, executes, and inserts a predefined set of blocks.
/// - It then verifies that the resulting blockchain state (post-state) matches the expected
///   outcome.
///
/// Returns:
/// - `Ok(())` if all blocks execute successfully.
/// - `Err(Error)` if any block fails to execute correctly.
fn run_case(case: &BlockchainTest, last_block_hash: &mut B256) -> Result<(), Error> {
    // Create a new test database and initialize a provider for the test case.
    let chain_spec = case.network.to_chain_spec();
    let factory = create_test_provider_factory(chain_spec.clone());
    let provider = factory.database_provider_rw().unwrap();

    // Insert initial test state into the provider.
    let genesis_block = SealedBlock::<Block>::from_sealed_parts(
        case.genesis_block_header.clone().into(),
        Default::default(),
    )
    .try_recover()
    .unwrap();

    provider.insert_block(&genesis_block).map_err(|err| Error::block_failed(0, err))?;
    *last_block_hash = genesis_block.hash();

    // Increment block number for receipts static file
    provider
        .static_file_provider()
        .latest_writer(StaticFileSegment::Receipts)
        .and_then(|mut writer| writer.increment_block(0))
        .map_err(|err| Error::block_failed(0, err))?;

    let genesis_state = case.pre.clone().into_genesis_state();
    insert_genesis_state(&provider, genesis_state.iter())
        .map_err(|err| Error::block_failed(0, err))?;
    insert_genesis_hashes(&provider, genesis_state.iter())
        .map_err(|err| Error::block_failed(0, err))?;
    insert_genesis_history(&provider, genesis_state.iter())
        .map_err(|err| Error::block_failed(0, err))?;

    // Build the genesis trie, as `init_genesis` does, so block 1 reads stored nodes.
    let (_, trie_updates) = reth_trie_db::with_adapter!(provider, |A| {
        StateRoot::<reth_trie_db::DatabaseTrieCursorFactory<_, A>, _>::from_tx(provider.tx_ref())
            .root_with_updates()
    })
    .map_err(|err| Error::block_failed(0, err))?;
    provider.write_trie_updates(trie_updates).map_err(|err| Error::block_failed(0, err))?;

    let executor_provider = EthEvmConfig::ethereum(chain_spec.clone());
    let mut parent = genesis_block;
    let mut bal_buf = Vec::new();

    for (block_index, fixture_block) in case.blocks.iter().enumerate() {
        // The blocks do not include the genesis block which is why we have the plus one.
        // We also cannot use block.number because for invalid blocks, this may be incorrect.
        let block_number = (block_index + 1) as u64;
        // Decoded one at a time, so that the blocks before one that does not decode are imported.
        let block = &decode_block(fixture_block, block_number)?;

        // Insert the block into the database
        provider.insert_block(block).map_err(|err| Error::block_failed(block_number, err))?;
        provider
            .static_file_provider()
            .commit()
            .map_err(|err| Error::block_failed(block_number, err))?;

        // Consensus checks before block execution
        pre_execution_checks(chain_spec.clone(), &parent, block)
            .map_err(|err| Error::block_failed(block_number, err))?;

        // Execute the block
        let state_provider = provider.latest();
        let state_db = StateProviderDatabase((&state_provider).into_evm_state_provider());
        let mut executor = executor_provider.batch_executor(state_db);

        let result = executor
            .execute_one(&(*block).clone())
            .map_err(|err| Error::block_failed(block_number, err))?;
        // Check the block access list size cap and compute its hash for post-Amsterdam blocks so
        // the consensus check below validates it.
        let block_access_list_hash = executor
            .take_bal()
            .map(|bal| {
                let bal = Bal::from(bal);
                bal.validate_gas_limit(block.gas_limit)
                    .map(|()| bal.compute_hash_with_buf(&mut bal_buf))
            })
            .transpose()
            .map_err(|err| Error::block_failed(block_number, err))?;
        let output = BlockExecutionOutput { state: executor.into_state(), result };

        // Consensus checks after block execution
        validate_block_post_execution(block, &chain_spec, &output, None, block_access_list_hash)
            .map_err(|err| Error::block_failed(block_number, err))?;

        // Compute and check the post state root
        let hashed_state = state_provider
            .hashed_post_state(&output.state)
            .map_err(|err| Error::block_failed(block_number, err))?;
        let sorted = hashed_state.clone_into_sorted();
        let (computed_state_root, trie_updates) = reth_trie_db::with_adapter!(provider, |A| {
            StateRoot::<reth_trie_db::DatabaseTrieCursorFactory<_, A>, _>::overlay_root_with_updates(
                provider.tx_ref(),
                &sorted,
            )
        })
        .map_err(|err| Error::block_failed(block_number, err))?;
        if computed_state_root != block.state_root {
            return Err(Error::block_failed(
                block_number,
                ConsensusError::BodyStateRootDiff(
                    GotExpected { got: computed_state_root, expected: block.state_root }.into(),
                ),
            ));
        }

        // Commit the post state/state diff to the database
        provider
            .write_state(
                &ExecutionOutcome::single(block.number, output),
                OriginalValuesKnown::Yes,
                StateWriteConfig::default(),
            )
            .map_err(|err| Error::block_failed(block_number, err))?;

        provider
            .write_hashed_state(&hashed_state.into_sorted())
            .map_err(|err| Error::block_failed(block_number, err))?;
        // Persist the trie so later blocks read stored nodes.
        provider
            .write_trie_updates(trie_updates)
            .map_err(|err| Error::block_failed(block_number, err))?;
        provider
            .update_history_indices(block.number..=block.number)
            .map_err(|err| Error::block_failed(block_number, err))?;

        // Since there were no errors, update the parent block
        *last_block_hash = block.hash();
        parent = block.clone()
    }

    match &case.post_state {
        Some(expected_post_state) => {
            // Validate the post-state for the test case.
            //
            // If we get here then it means that the post-state root checks
            // made after we execute each block was successful.
            //
            // If an error occurs here, then it is:
            // - Either an issue with the test setup
            // - Possibly an error in the test case where the post-state root in the last block does
            //   not match the post-state values.
            for (address, account) in expected_post_state {
                account.assert_db(*address, provider.tx_ref())?;
            }
        }
        None => {
            // Some tests may not have post-state (e.g., state-heavy benchmark tests).
            // In this case, we can skip the post-state validation.
        }
    }

    Ok(())
}

/// Checks that the chain ends on the fixture's `lastblockhash`, whether or not a block was
/// rejected.
fn check_last_block_hash(case: &BlockchainTest, last_block_hash: B256) -> Result<(), Error> {
    if last_block_hash == case.lastblockhash {
        return Ok(())
    }
    Err(Error::Assertion(format!(
        "last block hash mismatch: got {last_block_hash}, expected {}",
        case.lastblockhash
    )))
}

/// The rejection of the fixture block with the given number, counted from 1, with its hash if
/// it decodes.
fn rejection(case: &BlockchainTest, block_number: u64, error: String) -> Rejection {
    let index = (block_number - 1) as usize;
    let hash = case
        .blocks
        .get(index)
        .and_then(|block| SealedBlock::<Block>::decode(&mut block.rlp.as_ref()).ok())
        .map(|block| block.hash());
    Rejection { index, hash, error }
}

/// Decodes the fixture block with the given number, counted from 1, and recovers its senders.
fn decode_block(
    block: &crate::models::Block,
    block_number: u64,
) -> Result<RecoveredBlock<Block>, Error> {
    SealedBlock::<Block>::decode(&mut block.rlp.as_ref())
        .map_err(|err| Error::block_failed(block_number, err))?
        .try_recover()
        .map_err(|err| Error::block_failed(block_number, err))
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
        // exception in testsuite. There are more nonce overflow tests that are internal
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

/// `RocksDB` instance shared by all test cases of the current process.
///
/// Opening `RocksDB` is expensive and ef-tests never use it: the storage settings stay at v1, so
/// everything is stored in MDBX and static files. A fresh instance per case dominates the runtime.
///
/// The backing directory is created with [`reth_db::test_utils::tempdir_path`]. Statics are never
/// dropped, so the instance is never closed and no close can run while the process exits. A test
/// process leaves its small, mostly empty directory in the system temp dir; `ef-test-runner`
/// removes it with [`remove_shared_rocksdb`] once every fixture ran.
static SHARED_ROCKSDB: OnceLock<(PathBuf, RocksDBProvider)> = OnceLock::new();

/// Returns the [`SHARED_ROCKSDB`] instance, created on first use.
pub fn shared_rocksdb() -> RocksDBProvider {
    let (_, rocksdb) = SHARED_ROCKSDB.get_or_init(|| {
        let path = reth_db::test_utils::tempdir_path();
        let rocksdb = RocksDBBuilder::new(path.join("rocksdb"))
            .with_default_tables()
            .build()
            .expect("failed to create shared test RocksDB provider");
        (path, rocksdb)
    });
    rocksdb.clone()
}

/// Removes the directory of the [`SHARED_ROCKSDB`] instance, if one was created. Call it only once
/// no case runs anymore.
pub fn remove_shared_rocksdb() {
    if let Some((path, _)) = SHARED_ROCKSDB.get() &&
        let Err(err) = fs::remove_dir_all(path)
    {
        eprintln!("failed to remove {}: {err}", path.display());
    }
}

/// Creates a provider factory for a single test case.
///
/// Mirrors [`reth_provider::test_utils::create_test_provider_factory_with_chain_spec`], except
/// that it reuses [`SHARED_ROCKSDB`] instead of opening a new `RocksDB` instance. The temp datadir,
/// MDBX, static files and runtime stay isolated per case.
fn create_test_provider_factory(
    chain_spec: Arc<ChainSpec>,
) -> ProviderFactory<MockNodeTypesWithDB> {
    let genesis_block_number = chain_spec.genesis.number.unwrap_or_default();

    // A single temp directory holds the MDBX and static file dirs. `TempDatabase` removes the
    // entire directory on drop.
    let datadir_path = reth_db::test_utils::tempdir_path();
    let static_files_path = datadir_path.join("static_files");
    fs::create_dir_all(&static_files_path).expect("failed to create static_files dir");

    let db = reth_db::test_utils::create_test_rw_db_with_datadir(&datadir_path);

    let factory = ProviderFactory::new(
        db,
        chain_spec,
        StaticFileProviderBuilder::read_write(static_files_path)
            .with_genesis_block_number(genesis_block_number)
            .build()
            .expect("static file provider"),
        shared_rocksdb(),
        Runtime::test(),
    )
    .expect("failed to create test provider factory");

    assert!(
        !factory.cached_storage_settings().any_in_rocksdb(),
        "the shared RocksDB is only valid while ef-tests run with v1 storage settings"
    );

    factory
}
