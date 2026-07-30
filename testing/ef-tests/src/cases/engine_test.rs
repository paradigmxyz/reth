//! Test runners for `blockchain_test_engine` fixtures.
//!
//! These tests exercise the Engine API path by sending payloads through
//! `EngineApiTreeHandler::on_engine_message()`, receiving real `PayloadStatus`
//! (VALID/INVALID/SYNCING) responses, and following up with
//! `forkchoiceUpdated` for valid blocks.

use crate::{
    case::Cases,
    models::{EngineNewPayload, EngineTest, ForkSpec},
    result::{categorize_results, print_results, CaseResult},
    Case, Error,
};
use alloy_eips::eip7685::{Requests, RequestsOrHash};
use alloy_primitives::{keccak256, Bytes, B256};
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadSidecar,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ForkchoiceState,
    PayloadAttributes as EthPayloadAttributes, PraguePayloadFields,
};
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use reth_chain_state::{
    CanonicalInMemoryState, ComputedTrieData, DeferredTrieData, ExecutedBlock,
};
use reth_chainspec::ChainSpec;
use reth_consensus::{FullConsensus, HeaderValidator};
use reth_engine_primitives::{
    BeaconEngineMessage, ForkchoiceStatus, TreeConfig,
};
use reth_engine_tree::{
    engine::{EngineApiKind, FromEngine},
    persistence::PersistenceHandle,
    tree::{
        error::{InsertBlockError, InsertBlockErrorKind, InsertPayloadError},
        payload_validator::{EngineValidator, TreeCtx, ValidationOutcome},
        CacheWaitDurations, EngineApiTreeHandler, EngineApiTreeState,
        PersistenceState, WaitForCaches,
    },
};
use reth_ethereum_consensus::{
    validate_block_post_execution, EthBeaconConsensus,
};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_evm_ethereum::{EthEvmConfig, MockEvmConfig};
use reth_node_ethereum::EthereumEngineValidator;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{
    validate_execution_requests, validate_version_specific_fields,
    EngineApiMessageVersion, EngineObjectValidationError,
    InvalidPayloadAttributesError, NewPayloadError, PayloadOrAttributes,
};
use reth_primitives_traits::{
    Account, ParallelBridgeBuffered, RecoveredBlock, SealedBlock, SealedHeader,
};
use reth_provider::{
    test_utils::{ExtendedAccount, MockEthProvider},
};
use reth_revm::{database::StateProviderDatabase, test_utils::StateProviderTest};
use reth_trie_db::ChangesetCache;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use walkdir::{DirEntry, WalkDir};

/// Engine validator for ef-tests that performs real EVM execution.
///
/// This validator converts payloads to blocks via `EthereumEngineValidator`,
/// executes them through `EthEvmConfig`, performs consensus checks, and returns
/// `ExecutedBlock` results to the engine tree. State root is trusted from the
/// block header (the fixture guarantees correctness) rather than computed via
/// the trie, because `MockEthProvider` has no trie storage.
struct EfTestEngineValidator {
    chain_spec: Arc<ChainSpec>,
    evm_config: EthEvmConfig,
    inner_validator: EthereumEngineValidator,
    consensus: Arc<EthBeaconConsensus<ChainSpec>>,
    /// In-memory state provider for EVM execution, shared with the test
    /// harness via `Arc` so post-state can be verified after all payloads
    /// are processed.
    state: Arc<std::sync::Mutex<StateProviderTest>>,
    /// The engine tree's `MockEthProvider` — shared via `Arc` internals so
    /// that blocks/headers added here are visible to the tree.
    tree_provider: MockEthProvider,
}

impl EfTestEngineValidator {
    fn new(
        chain_spec: Arc<ChainSpec>,
        state: Arc<std::sync::Mutex<StateProviderTest>>,
        tree_provider: MockEthProvider,
    ) -> Self {
        let evm_config = EthEvmConfig::new(chain_spec.clone());
        let inner_validator = EthereumEngineValidator::new(chain_spec.clone());
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));
        Self {
            chain_spec,
            evm_config,
            inner_validator,
            consensus,
            state,
            tree_provider,
        }
    }

    /// Execute a block against the in-memory state and return an `ExecutedBlock`.
    fn execute_block(
        &self,
        block: &RecoveredBlock<Block>,
    ) -> Result<ExecutedBlock, InsertBlockErrorKind> {
        let mut state_guard = self.state.lock().unwrap();

        let state_db = StateProviderDatabase::new(&*state_guard);
        let executor = self.evm_config.batch_executor(state_db);

        let output = executor.execute(block).map_err(|e| {
            InsertBlockErrorKind::Other(Box::new(e))
        })?;

        // Post-execution consensus checks
        validate_block_post_execution(
            block,
            &self.chain_spec,
            &output.receipts,
            &output.requests,
            None,
        )
        .map_err(InsertBlockErrorKind::Consensus)?;

        // Apply state changes for subsequent blocks
        state_guard.apply_bundle_state(&output.state);
        state_guard.insert_block_hash(block.number, block.hash());

        // Also record in the tree provider so the engine tree can find parents
        self.tree_provider.add_block(block.hash(), block.clone_block());
        self.tree_provider.add_header(block.hash(), block.header().clone());

        Ok(ExecutedBlock {
            recovered_block: Arc::new(block.clone()),
            execution_output: Arc::new(output),
            trie_data: DeferredTrieData::ready(ComputedTrieData::default()),
        })
    }
}

impl std::fmt::Debug for EfTestEngineValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EfTestEngineValidator").finish_non_exhaustive()
    }
}

impl reth_engine_primitives::PayloadValidator<EthEngineTypes> for EfTestEngineValidator {
    type Block = Block;

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block>, NewPayloadError> {
        <EthereumEngineValidator as reth_engine_primitives::PayloadValidator<EthEngineTypes>>::convert_payload_to_block(
            &self.inner_validator, payload,
        )
    }
}

impl reth_engine_primitives::EngineApiValidator<EthEngineTypes> for EfTestEngineValidator {
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, ExecutionData, EthPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        <EthereumEngineValidator as reth_engine_primitives::EngineApiValidator<EthEngineTypes>>::validate_version_specific_fields(
            &self.inner_validator, version, payload_or_attrs,
        )
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &EthPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        <EthereumEngineValidator as reth_engine_primitives::EngineApiValidator<EthEngineTypes>>::ensure_well_formed_attributes(
            &self.inner_validator, version, attributes,
        )
    }
}

impl EngineValidator<EthEngineTypes> for EfTestEngineValidator {
    fn validate_payload_attributes_against_header(
        &self,
        _attr: &EthPayloadAttributes,
        _header: &alloy_consensus::Header,
    ) -> Result<(), InvalidPayloadAttributesError> {
        Ok(())
    }

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block>, NewPayloadError> {
        <Self as reth_engine_primitives::PayloadValidator<EthEngineTypes>>::convert_payload_to_block(
            self, payload,
        )
    }

    fn validate_payload(
        &mut self,
        payload: ExecutionData,
        _ctx: TreeCtx<'_, EthPrimitives>,
    ) -> ValidationOutcome<EthPrimitives> {
        // Convert payload to sealed block
        let sealed_block = <EthereumEngineValidator as reth_engine_primitives::PayloadValidator<EthEngineTypes>>::convert_payload_to_block(
            &self.inner_validator, payload,
        ).map_err(InsertPayloadError::Payload)?;

        let block = sealed_block.clone().try_recover().map_err(|e| {
            InsertBlockError::new(
                sealed_block.clone(),
                InsertBlockErrorKind::Other(Box::new(e)),
            )
        })?;

        // Consensus pre-execution checks
        {
            use reth_consensus::{Consensus, HeaderValidator};
            let sealed_header = block.sealed_header();
            <EthBeaconConsensus<ChainSpec> as Consensus<Block>>::validate_body_against_header(
                &self.consensus,
                block.body(),
                sealed_header,
            )
            .map_err(|e| {
                InsertBlockError::new(
                    sealed_block.clone(),
                    InsertBlockErrorKind::Consensus(e),
                )
            })?;
            self.consensus.validate_header(sealed_header).map_err(|e| {
                InsertBlockError::new(
                    sealed_block.clone(),
                    InsertBlockErrorKind::Consensus(e),
                )
            })?;
            self.consensus.validate_block_pre_execution(&block).map_err(|e| {
                InsertBlockError::new(
                    sealed_block.clone(),
                    InsertBlockErrorKind::Consensus(e),
                )
            })?;
        }

        // Execute block through the real EVM.
        // execute_block handles state application and block/header recording.
        let executed = self.execute_block(&block).map_err(|kind| {
            InsertBlockError::new(sealed_block, kind)
        })?;

        Ok((executed, None))
    }

    fn validate_block(
        &mut self,
        _block: SealedBlock<Block>,
        _ctx: TreeCtx<'_, EthPrimitives>,
    ) -> ValidationOutcome<EthPrimitives> {
        // Not called for engine test payloads — only for downloaded blocks.
        unimplemented!("validate_block not used in engine test runner")
    }

    fn on_inserted_executed_block(&self, _block: ExecutedBlock) {
        // no-op
    }
}

impl WaitForCaches for EfTestEngineValidator {
    fn wait_for_caches(&self) -> CacheWaitDurations {
        CacheWaitDurations { execution_cache: Duration::ZERO, sparse_trie: Duration::ZERO }
    }
}

use std::sync::atomic::{AtomicU8, Ordering};

static ENGINE_TEST_MODE: AtomicU8 = AtomicU8::new(1); // 1 = EngineTree (default)

/// Engine test execution mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EngineTestMode {
    /// Fast mode: direct EVM execution with parameter validation.
    /// Skips engine tree and state root verification. (~5s for 40k tests)
    Fast = 0,
    /// Default: real engine tree with in-memory provider.
    /// Sends payloads through on_new_payload → PayloadStatus → FCU. (~37s for 40k tests)
    #[default]
    EngineTree = 1,
}

impl EngineTestMode {
    fn set_global(self) {
        ENGINE_TEST_MODE.store(self as u8, Ordering::Relaxed);
    }

    fn get_global() -> Self {
        match ENGINE_TEST_MODE.load(Ordering::Relaxed) {
            0 => Self::Fast,
            _ => Self::EngineTree,
        }
    }
}

/// A handler for the engine test suite.
#[derive(Debug)]
pub struct EngineTests {
    suite_path: PathBuf,
    mode: EngineTestMode,
}

impl EngineTests {
    /// Create a new suite for engine test fixtures.
    pub fn new(suite_path: PathBuf) -> Self {
        Self { suite_path, mode: EngineTestMode::default() }
    }

    /// Set the execution mode.
    pub fn with_mode(mut self, mode: EngineTestMode) -> Self {
        self.mode = mode;
        self
    }
}

impl EngineTests {
    /// Walk the suite path and run all engine test fixtures found.
    ///
    /// Unlike the default `Suite::run`, this handles two directory layouts:
    ///   1. Path contains subdirectories (each with JSON files) -- iterate subs.
    ///   2. Path directly contains JSON files -- run them as a single batch.
    pub fn run(&self) {
        let suite_path = &self.suite_path;

        // Check if the path contains subdirectories
        let has_subdirs = WalkDir::new(suite_path)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .any(|e| e.file_type().is_dir());

        if has_subdirs {
            // Standard layout: iterate over each subdirectory
            for entry in WalkDir::new(suite_path).min_depth(1).max_depth(1) {
                let entry = entry.expect("Failed to read directory");
                if entry.file_type().is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    self.run_dir(&name, &entry.into_path());
                }
            }
        } else {
            // Flat layout: JSON files directly in suite_path
            let name = suite_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "engine_tests".to_string());
            self.run_dir(&name, suite_path);
        }
    }

    /// Run all tests and print results as a JSON array.
    pub fn run_json(&self) {
        let results = self.collect_all();
        crate::result::print_json_array(&results);
    }

    /// Collect per-test results for JSON output.
    fn collect_all(&self) -> Vec<CaseResult> {
        use rayon::prelude::*;

        let suite_path = &self.suite_path;
        self.mode.set_global();

        // Load all files, then flatten into (path, test_name, test) triples
        let test_items: Vec<_> = find_all_json_files(suite_path)
            .into_iter()
            .filter_map(|path| {
                let case = EngineTestCase::load(&path).ok()?;
                Some(
                    case.tests
                        .into_iter()
                        .filter(|(_, t)| !EngineTestCase::excluded_fork(t.network))
                        .map(move |(name, test)| (path.clone(), name, test))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();

        test_items
            .into_par_iter()
            .map(|(path, name, test)| {
                let result = EngineTestCase::run_single_case(&name, &test);
                CaseResult::new(&path, name, result)
            })
            .collect()
    }

    /// Load and run all JSON test files in the given directory (recursively).
    fn run_dir(&self, name: &str, dir: &Path) {
        assert!(dir.exists(), "Test suite path does not exist: {dir:?}");

        self.mode.set_global();

        let test_cases: Vec<_> = find_all_json_files(dir)
            .into_iter()
            .map(|path| {
                let case =
                    EngineTestCase::load(&path).expect("engine test case should load");
                (path, case)
            })
            .collect();

        let results = Cases { test_cases }.run();

        let (passed, failed, skipped) = categorize_results(&results);
        print_results(name, dir, &passed, &failed, &skipped);
    }
}

/// Recursively find all `.json` files under the given path.
fn find_all_json_files(path: &Path) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .map(DirEntry::into_path)
        .collect()
}

/// An Ethereum engine test case.
#[derive(Debug, PartialEq, Eq)]
pub struct EngineTestCase {
    /// The tests within this test case file.
    pub tests: BTreeMap<String, EngineTest>,
    /// Whether to skip this test case.
    pub skip: bool,
}

impl EngineTestCase {
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

    /// Execute a single `EngineTest`, validating the outcome against the
    /// expectations encoded in the JSON file.
    pub fn run_single_case(name: &str, case: &EngineTest) -> Result<(), Error> {
        match EngineTestMode::get_global() {
            EngineTestMode::Fast => run_engine_case_fast(name, case),
            EngineTestMode::EngineTree => run_engine_case(name, case),
        }
    }
}

impl Case for EngineTestCase {
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

    fn run(self) -> Result<(), Error> {
        if self.skip {
            return Err(Error::Skipped);
        }

        self.tests
            .into_iter()
            .filter(|(_, case)| !Self::excluded_fork(case.network))
            .par_bridge_buffered()
            .with_min_len(64)
            .try_for_each(|(name, case)| Self::run_single_case(&name, &case).map(|_| ()))
    }
}

/// Maps the fixture's `newPayloadVersion` integer to an [`EngineApiMessageVersion`].
fn to_engine_version(version: u64) -> Result<EngineApiMessageVersion, Error> {
    match version {
        1 => Ok(EngineApiMessageVersion::V1),
        2 => Ok(EngineApiMessageVersion::V2),
        3 => Ok(EngineApiMessageVersion::V3),
        4 => Ok(EngineApiMessageVersion::V4),
        5 => Ok(EngineApiMessageVersion::V5),
        other => Err(Error::Assertion(format!(
            "Unsupported newPayloadVersion: {other}"
        ))),
    }
}

/// Parse the `params` array from an [`EngineNewPayload`] and construct an
/// [`ExecutionData`] (payload + sidecar) suitable for validation.
///
/// The params layout is:
///   [0] ExecutionPayloadV3 (JSON object)
///   [1] versioned_hashes (Vec<B256>)
///   [2] parent_beacon_block_root (B256)
///   [3] execution_requests (Vec<Bytes>)  -- V4 only
fn parse_execution_data(entry: &EngineNewPayload) -> Result<ExecutionData, Error> {
    if entry.params.is_empty() {
        return Err(Error::Assertion("engineNewPayloads params is empty".into()));
    }

    // params[0]: ExecutionPayload — version-aware deserialization
    let payload = match entry.new_payload_version {
        1 => {
            let v1: ExecutionPayloadV1 =
                serde_json::from_value(entry.params[0].clone()).map_err(|e| {
                    Error::Assertion(format!("Failed to deserialize ExecutionPayloadV1: {e}"))
                })?;
            ExecutionPayload::V1(v1)
        }
        2 => {
            let v2: ExecutionPayloadV2 =
                serde_json::from_value(entry.params[0].clone()).map_err(|e| {
                    Error::Assertion(format!("Failed to deserialize ExecutionPayloadV2: {e}"))
                })?;
            ExecutionPayload::V2(v2)
        }
        _ => {
            // V3, V4, V5 all use the V3 payload structure
            let v3: ExecutionPayloadV3 =
                serde_json::from_value(entry.params[0].clone()).map_err(|e| {
                    Error::Assertion(format!("Failed to deserialize ExecutionPayloadV3: {e}"))
                })?;
            ExecutionPayload::V3(v3)
        }
    };

    // params[1]: versioned_hashes (may be absent for V1/V2)
    let versioned_hashes: Vec<B256> = if entry.params.len() > 1 {
        serde_json::from_value(entry.params[1].clone()).map_err(|e| {
            Error::Assertion(format!("Failed to deserialize versioned_hashes: {e}"))
        })?
    } else {
        vec![]
    };

    // params[2]: parent_beacon_block_root (may be absent for V1/V2)
    let parent_beacon_block_root: B256 = if entry.params.len() > 2 {
        serde_json::from_value(entry.params[2].clone()).map_err(|e| {
            Error::Assertion(format!(
                "Failed to deserialize parent_beacon_block_root: {e}"
            ))
        })?
    } else {
        B256::ZERO
    };

    // Build the sidecar based on the payload version
    let sidecar = if entry.new_payload_version >= 4 && entry.params.len() > 3 {
        // params[3]: execution_requests
        let requests_bytes: Vec<Bytes> =
            serde_json::from_value(entry.params[3].clone()).map_err(|e| {
                Error::Assertion(format!("Failed to deserialize execution_requests: {e}"))
            })?;
        let requests = Requests::from_requests(requests_bytes);
        let cancun = CancunPayloadFields {
            parent_beacon_block_root,
            versioned_hashes,
        };
        let prague = PraguePayloadFields::new(RequestsOrHash::Requests(requests));
        ExecutionPayloadSidecar::v4(cancun, prague)
    } else if entry.new_payload_version >= 3 {
        let cancun = CancunPayloadFields {
            parent_beacon_block_root,
            versioned_hashes,
        };
        ExecutionPayloadSidecar::v3(cancun)
    } else {
        ExecutionPayloadSidecar::none()
    };

    Ok(ExecutionData { payload, sidecar })
}

/// The engine tree type used by ef-tests.
type EfTestEngineTree = EngineApiTreeHandler<
    EthPrimitives,
    MockEthProvider,
    EthEngineTypes,
    EfTestEngineValidator,
    MockEvmConfig,
>;

/// Create the engine tree harness for a test case.
///
/// Returns the tree handler and the `StateProviderTest` (for post-state
/// verification — the `StateProviderTest` is shared with the validator via
/// the `Mutex` inside `EfTestEngineValidator`).
fn create_engine_tree(
    chain_spec: Arc<ChainSpec>,
    case: &EngineTest,
) -> Result<(EfTestEngineTree, Arc<std::sync::Mutex<StateProviderTest>>), Error> {
    use std::sync::mpsc::channel;

    // ---- Build the in-memory state provider for EVM execution ----
    let mut state_provider = StateProviderTest::default();
    for (address, account) in case.pre.iter() {
        let storage = account
            .storage
            .iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(k, v)| (B256::from(*k), *v))
            .collect();
        let reth_account = Account {
            nonce: account.nonce.to::<u64>(),
            balance: account.balance,
            bytecode_hash: if account.code.is_empty() {
                None
            } else {
                Some(keccak256(&account.code))
            },
        };
        let bytecode =
            if account.code.is_empty() { None } else { Some(account.code.clone()) };
        state_provider.insert_account(*address, reth_account, bytecode, storage);
    }

    // Insert genesis block hash so BLOCKHASH opcode works for block 0.
    let sealed_genesis_header: SealedHeader = case.genesis_block_header.clone().into();
    let genesis_block = SealedBlock::<Block>::from_sealed_parts(
        sealed_genesis_header.clone(),
        Default::default(),
    );
    let genesis_hash = genesis_block.hash();
    state_provider.insert_block_hash(0, genesis_hash);

    // ---- Build the MockEthProvider for the engine tree ----
    // The tree needs this to look up parent blocks/headers.
    let tree_provider =
        MockEthProvider::default().with_chain_spec((*chain_spec).clone());
    tree_provider.add_block(genesis_hash, genesis_block.clone_block());
    tree_provider.add_header(genesis_hash, sealed_genesis_header.header().clone());

    // Also add genesis accounts to MockEthProvider so that
    // state_by_block_hash works for the genesis parent lookup.
    for (address, account) in case.pre.iter() {
        let mut ext = ExtendedAccount::new(account.nonce.to::<u64>(), account.balance);
        if !account.code.is_empty() {
            ext = ext.with_bytecode(account.code.clone());
        }
        let storage: Vec<_> = account
            .storage
            .iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(k, v)| (B256::from(*k), *v))
            .collect();
        if !storage.is_empty() {
            ext = ext.extend_storage(storage);
        }
        tree_provider.add_account(*address, ext);
    }

    // ---- Create the shared state handle ----
    let shared_state = Arc::new(std::sync::Mutex::new(state_provider));

    // ---- Create the engine validator with real EVM ----
    let engine_validator = EfTestEngineValidator::new(
        chain_spec.clone(),
        shared_state.clone(),
        tree_provider.clone(),
    );

    // ---- Assemble the engine tree ----
    let (action_tx, _action_rx) = channel();
    let persistence_handle = PersistenceHandle::new(action_tx);

    let consensus: Arc<dyn FullConsensus<EthPrimitives>> =
        Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

    let (from_tree_tx, _from_tree_rx) = unbounded_channel();

    let engine_api_tree_state = EngineApiTreeState::new(
        10,
        10,
        sealed_genesis_header.num_hash(),
        EngineApiKind::Ethereum,
    );
    let canonical_in_memory_state =
        CanonicalInMemoryState::with_head(sealed_genesis_header, None, None);

    let (to_payload_service, _payload_command_rx) = unbounded_channel();
    let payload_builder =
        PayloadBuilderHandle::<EthEngineTypes>::new(to_payload_service);

    let evm_config = MockEvmConfig::default();
    let changeset_cache = ChangesetCache::new();
    let runtime = reth_tasks::Runtime::test();

    let tree = EngineApiTreeHandler::new(
        tree_provider.clone(),
        consensus,
        engine_validator,
        from_tree_tx,
        engine_api_tree_state,
        canonical_in_memory_state,
        persistence_handle,
        PersistenceState::default(),
        payload_builder,
        TreeConfig::default()
            .with_state_root_fallback(true)
            .with_legacy_state_root(true),
        EngineApiKind::Ethereum,
        evm_config,
        changeset_cache,
        runtime,
    );

    Ok((tree, shared_state))
}

/// Fast mode: validates Engine API params, converts payload, executes via direct
/// EVM, verifies post-state accounts. Skips engine tree and state root.
fn run_engine_case_fast(name: &str, case: &EngineTest) -> Result<(), Error> {
    let chain_spec = case.network.to_chain_spec();
    let payload_validator = EthereumExecutionPayloadValidator::new(chain_spec.clone());
    let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

    // Build in-memory state from fixture pre-state
    let mut state_provider = StateProviderTest::default();
    for (address, account) in case.pre.iter() {
        let storage = account.storage.iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(k, v)| (B256::from(*k), *v))
            .collect();
        let reth_account = Account {
            nonce: account.nonce.to::<u64>(),
            balance: account.balance,
            bytecode_hash: if account.code.is_empty() { None } else { Some(keccak256(&account.code)) },
        };
        let bytecode = if account.code.is_empty() { None } else { Some(account.code.clone()) };
        state_provider.insert_account(*address, reth_account, bytecode, storage);
    }

    let genesis_block = SealedBlock::<Block>::from_sealed_parts(
        case.genesis_block_header.clone().into(), Default::default(),
    ).try_recover().unwrap();
    state_provider.insert_block_hash(0, genesis_block.hash());

    let executor_provider = EthEvmConfig::ethereum(chain_spec.clone());
    let mut parent = genesis_block;

    for (payload_idx, entry) in case.engine_new_payloads.iter().enumerate() {
        let expects_error = entry.validation_error.is_some() || entry.error_code.is_some();

        let execution_data = match parse_execution_data(entry) {
            Ok(data) => data,
            Err(err) => { if expects_error { continue; } return Err(err); }
        };

        // Version validation (same as RPC layer)
        let engine_version = to_engine_version(entry.new_payload_version)?;
        let payload_or_attrs = PayloadOrAttributes::<'_, ExecutionData, EthPayloadAttributes>::from_execution_payload(&execution_data);
        if let Some(requests) = payload_or_attrs.execution_requests() {
            if let Err(_err) = validate_execution_requests(requests) {
                if expects_error { continue; }
                return Err(Error::Assertion(format!("Test case: {name}\nPayload {payload_idx}: unexpected execution requests validation error: {_err}")));
            }
        }
        if let Err(_err) = validate_version_specific_fields(chain_spec.as_ref(), engine_version, payload_or_attrs) {
            if expects_error { continue; }
            return Err(Error::Assertion(format!("Test case: {name}\nPayload {payload_idx}: unexpected version validation error: {_err}")));
        }

        // Convert payload to block
        let sealed_block = match payload_validator.ensure_well_formed_payload(execution_data) {
            Ok(block) => block,
            Err(err) => { if expects_error { continue; } return Err(Error::Assertion(format!("Test case: {name}\nPayload {payload_idx}: payload validation error: {err}"))); }
        };
        let block = match sealed_block.try_recover() {
            Ok(b) => b,
            Err(err) => { if expects_error { continue; } return Err(Error::Assertion(format!("Test case: {name}\nPayload {payload_idx}: recovery error: {err}"))); }
        };

        // Consensus pre-checks
        if let Err(err) = consensus.validate_header_against_parent(block.sealed_header(), parent.sealed_header()) {
            if expects_error { continue; }
            return Err(Error::block_failed(payload_idx as u64 + 1, err));
        }

        // Execute
        let state_db = StateProviderDatabase::new(&state_provider);
        let executor = executor_provider.batch_executor(state_db);
        let output = match executor.execute(&block) {
            Ok(output) => output,
            Err(err) => { if expects_error { continue; } return Err(Error::block_failed(payload_idx as u64 + 1, err)); }
        };

        // Post-execution checks
        if let Err(err) = validate_block_post_execution(&block, &chain_spec, &output.receipts, &output.requests, None) {
            if expects_error { continue; }
            return Err(Error::block_failed(payload_idx as u64 + 1, err));
        }

        if expects_error {
            return Err(Error::Assertion(format!("Test case: {name}\nPayload {payload_idx}: expected error ({:?} / error_code {:?}) but payload was accepted", entry.validation_error, entry.error_code)));
        }

        state_provider.apply_bundle_state(&output.state);
        state_provider.insert_block_hash(payload_idx as u64 + 1, block.hash());
        parent = block;
    }

    // Verify post-state
    if let Some(ref post) = case.post_state {
        verify_post_state(&state_provider, post)?;
    }
    Ok(())
}

/// Executes a single engine test case through the real engine tree path.
///
/// For each payload in the fixture:
///   1. Validate version-specific fields (Engine API RPC layer check)
///   2. Send `BeaconEngineMessage::NewPayload` through `on_engine_message()`
///   3. Check the returned `PayloadStatus` against fixture expectations
///   4. For valid payloads: send `BeaconEngineMessage::ForkchoiceUpdated`
///
/// If the fixture specifies `validationError` or `errorCode`, we expect
/// INVALID or an RPC-layer error.
fn run_engine_case(name: &str, case: &EngineTest) -> Result<(), Error> {
    let chain_spec = case.network.to_chain_spec();

    let (mut tree, shared_state) = create_engine_tree(chain_spec.clone(), case)?;

    let mut last_valid_hash: Option<B256> = None;

    for (payload_idx, entry) in case.engine_new_payloads.iter().enumerate() {
        let expects_error = entry.validation_error.is_some() || entry.error_code.is_some();

        // -- Step 1: Parse the execution data from fixture params --
        let execution_data = match parse_execution_data(entry) {
            Ok(data) => data,
            Err(err) => {
                if expects_error {
                    continue;
                }
                return Err(err);
            }
        };

        // -- Step 2: Engine API version-specific validation (RPC layer) --
        // This catches errorCode=-32602 cases before hitting the engine tree.
        let engine_version = to_engine_version(entry.new_payload_version)?;

        let payload_or_attrs = PayloadOrAttributes::<
            '_,
            ExecutionData,
            EthPayloadAttributes,
        >::from_execution_payload(&execution_data);

        // Validate execution requests ordering
        if let Some(requests) = payload_or_attrs.execution_requests() {
            if let Err(_err) = validate_execution_requests(requests) {
                if expects_error {
                    continue;
                }
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: \
                     unexpected execution requests validation error: {_err}"
                )));
            }
        }

        if let Err(_err) = validate_version_specific_fields(
            chain_spec.as_ref(),
            engine_version,
            payload_or_attrs,
        ) {
            if expects_error {
                continue;
            }
            return Err(Error::Assertion(format!(
                "Test case: {name}\nPayload {payload_idx}: \
                 unexpected version validation error: {_err}"
            )));
        }

        // -- Step 3: Send NewPayload through the engine tree --
        let (tx, rx) = oneshot::channel();
        let msg = FromEngine::Request(
            BeaconEngineMessage::NewPayload {
                payload: execution_data,
                tx,
            }
            .into(),
        );

        if let Err(err) = tree.on_engine_message(msg) {
            if expects_error {
                continue;
            }
            return Err(Error::Assertion(format!(
                "Test case: {name}\nPayload {payload_idx}: \
                 engine tree fatal error: {err}"
            )));
        }

        let status = match rx.blocking_recv() {
            Ok(Ok(status)) => status,
            Ok(Err(err)) => {
                if expects_error {
                    continue;
                }
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: \
                     newPayload returned error: {err}"
                )));
            }
            Err(_) => {
                if expects_error {
                    continue;
                }
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: \
                     newPayload channel closed without response"
                )));
            }
        };

        // -- Step 4: Check payload status against expectations --
        if expects_error {
            // Fixture expects rejection. The payload should be INVALID.
            if status.is_valid() {
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: expected error \
                     ({:?} / error_code {:?}) but got VALID status",
                    entry.validation_error, entry.error_code,
                )));
            }
            // INVALID or SYNCING for an expected-error payload is fine
            continue;
        }

        // Fixture expects success — status must be VALID.
        if !status.is_valid() {
            return Err(Error::Assertion(format!(
                "Test case: {name}\nPayload {payload_idx}: expected VALID but got {status:?}"
            )));
        }

        let block_hash = status
            .latest_valid_hash
            .unwrap_or(last_valid_hash.unwrap_or(B256::ZERO));
        last_valid_hash = Some(block_hash);

        // -- Step 5: Send ForkchoiceUpdated for valid payloads --
        let fcu_state = ForkchoiceState {
            head_block_hash: block_hash,
            safe_block_hash: block_hash,
            finalized_block_hash: block_hash,
        };

        let (fcu_tx, fcu_rx) = oneshot::channel();
        let fcu_version = to_engine_version(entry.forkchoice_updated_version)?;
        let fcu_msg = FromEngine::Request(
            BeaconEngineMessage::ForkchoiceUpdated {
                state: fcu_state,
                payload_attrs: None,
                tx: fcu_tx,
                version: fcu_version,
            }
            .into(),
        );

        if let Err(err) = tree.on_engine_message(fcu_msg) {
            return Err(Error::Assertion(format!(
                "Test case: {name}\nPayload {payload_idx}: \
                 FCU engine tree fatal error: {err}"
            )));
        }

        match fcu_rx.blocking_recv() {
            Ok(Ok(on_fcu)) => {
                let fcu_status = on_fcu.forkchoice_status();
                if fcu_status != ForkchoiceStatus::Valid {
                    return Err(Error::Assertion(format!(
                        "Test case: {name}\nPayload {payload_idx}: \
                         FCU expected Valid but got {fcu_status:?}"
                    )));
                }
            }
            Ok(Err(err)) => {
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: \
                     FCU returned error: {err}"
                )));
            }
            Err(_) => {
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: \
                     FCU channel closed without response"
                )));
            }
        }
    }

    // -- Step 6: Validate final post-state --
    if let Some(ref expected_post_state) = case.post_state {
        let state_guard = shared_state.lock().unwrap();
        verify_post_state(&state_guard, expected_post_state)?;
    }

    Ok(())
}

/// Verify post-state accounts directly against the in-memory state provider.
fn verify_post_state(
    provider: &StateProviderTest,
    expected: &BTreeMap<alloy_primitives::Address, crate::models::Account>,
) -> Result<(), Error> {
    use reth_storage_api::{AccountReader, StateProvider};

    for (address, expected_account) in expected {
        let account = provider.basic_account(address).map_err(|e| {
            Error::Assertion(format!("Failed to read account {address}: {e}"))
        })?;

        let account = account.ok_or_else(|| {
            Error::Assertion(format!(
                "Expected account ({address}) is missing: {expected_account:?}"
            ))
        })?;

        // Verify balance
        if expected_account.balance != account.balance {
            return Err(Error::Assertion(format!(
                "Account {address}: balance mismatch: \
                 expected {}, got {}",
                expected_account.balance, account.balance
            )));
        }

        // Verify nonce
        let expected_nonce = expected_account.nonce.to::<u64>();
        if expected_nonce != account.nonce {
            return Err(Error::Assertion(format!(
                "Account {address}: nonce mismatch: \
                 expected {expected_nonce}, got {}",
                account.nonce
            )));
        }

        // Verify bytecode
        if expected_account.code.is_empty() {
            if account.bytecode_hash.is_some() {
                return Err(Error::Assertion(format!(
                    "Account {address}: expected no bytecode, \
                     but account has bytecode_hash {:?}",
                    account.bytecode_hash
                )));
            }
        } else {
            let expected_hash = keccak256(&expected_account.code);
            match account.bytecode_hash {
                Some(hash) if hash == expected_hash => {}
                Some(hash) => {
                    return Err(Error::Assertion(format!(
                        "Account {address}: bytecode hash mismatch: \
                         expected {expected_hash}, got {hash}"
                    )));
                }
                None => {
                    return Err(Error::Assertion(format!(
                        "Account {address}: expected bytecode hash \
                         {expected_hash}, but account has no bytecode"
                    )));
                }
            }
        }

        // Verify storage
        for (slot, expected_value) in &expected_account.storage {
            let key = B256::from(*slot);
            let actual = provider.storage(*address, key).map_err(|e| {
                Error::Assertion(format!(
                    "Failed to read storage {address}[{slot}]: {e}"
                ))
            })?;

            let actual_value = actual.unwrap_or(alloy_primitives::U256::ZERO);
            if *expected_value != actual_value {
                return Err(Error::Assertion(format!(
                    "Account {address}: storage slot {slot} mismatch: \
                     expected {expected_value}, got {actual_value}"
                )));
            }
        }
    }

    Ok(())
}

/// Returns whether the test at the given path should be skipped.
pub fn should_skip(_path: &Path) -> bool {
    // Engine tests do not have the same problematic edge cases as blockchain
    // tests (no RLP parsing, no uncle blocks). Skip nothing for now.
    false
}
