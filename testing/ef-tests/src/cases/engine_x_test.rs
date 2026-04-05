//! Test runner for `blockchain_test_engine_x` fixtures.
//!
//! The engine-x format splits pre-allocation from test definitions:
//!   - `pre_alloc/{hash}.json` — shared genesis state (24k+ files)
//!   - `{fork}/{testdir}/{test}.json` — test files with `preHash` referencing a pre-alloc
//!
//! Tests sharing the same `preHash` can reuse the same engine tree, amortizing
//! the expensive database creation. Engines are cached by `"fork:preHash"`.
//!
//! Uses `MockEthProvider` + `EngineApiTreeHandler` for fast in-memory execution.
//!
//! Access is serialized per engine via mutex to prevent concurrent test
//! interference. After each test, an FCU resets the chain head back to genesis.

use crate::{
    models::{Account, ForkSpec, Header},
    result::{categorize_results, print_results, CaseResult},
    Error,
};
use alloy_eips::eip7685::{Requests, RequestsOrHash};
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadSidecar,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ForkchoiceState,
    PayloadAttributes as EthPayloadAttributes, PraguePayloadFields,
};
use rayon::prelude::*;
use reth_chain_state::{CanonicalInMemoryState, ComputedTrieData, DeferredTrieData, ExecutedBlock};
use reth_chainspec::ChainSpec;
use reth_consensus::{Consensus, FullConsensus, HeaderValidator};
use reth_engine_primitives::{BeaconEngineMessage, ForkchoiceStatus, TreeConfig};
use reth_engine_tree::{
    engine::{EngineApiKind, FromEngine},
    persistence::PersistenceHandle,
    tree::{
        error::{InsertBlockError, InsertBlockErrorKind, InsertPayloadError},
        payload_validator::{EngineValidator, TreeCtx, ValidationOutcome},
        CacheWaitDurations, EngineApiTreeHandler, EngineApiTreeState, PersistenceState,
        WaitForCaches,
    },
};
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_evm_ethereum::{EthEvmConfig, MockEvmConfig};
use reth_node_ethereum::EthereumEngineValidator;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{
    validate_execution_requests, validate_version_specific_fields, EngineApiMessageVersion,
    EngineObjectValidationError, InvalidPayloadAttributesError, NewPayloadError,
    PayloadOrAttributes,
};
use reth_primitives_traits::{Account as RethAccount, RecoveredBlock, SealedBlock, SealedHeader};
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_revm::{database::StateProviderDatabase, test_utils::StateProviderTest};
use reth_trie_db::ChangesetCache;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Engine-x fixture types
// ---------------------------------------------------------------------------

/// Pre-allocation file: shared genesis state referenced by hash.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExPreAlloc {
    /// Genesis block header.
    genesis: Header,
    /// Pre-state accounts.
    pre: BTreeMap<alloy_primitives::Address, Account>,
    /// Network fork name (present in JSON; we use the test's network instead).
    #[allow(dead_code)]
    network: ForkSpec,
}

/// A single test definition in the engine-x format.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExTestDef {
    /// Network fork name.
    network: ForkSpec,
    /// Hash referencing a pre-alloc file.
    pre_hash: String,
    /// Engine new payload entries.
    engine_new_payloads: Vec<ExNewPayload>,
}

/// A single `engineNewPayloads` entry.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExNewPayload {
    /// Raw params array.
    params: Vec<serde_json::Value>,
    /// The `newPayload` version (e.g. 4).
    #[serde(deserialize_with = "crate::models::deserialize_string_u64")]
    new_payload_version: u64,
    /// The `forkchoiceUpdated` version (e.g. 3).
    #[serde(deserialize_with = "crate::models::deserialize_string_u64")]
    forkchoice_updated_version: u64,
    /// Expected validation error (empty string or absent means valid).
    #[serde(default)]
    validation_error: Option<String>,
    /// Expected JSON-RPC error code (e.g. -32602).
    #[serde(default, deserialize_with = "crate::models::deserialize_optional_string_i64")]
    error_code: Option<i64>,
}

impl ExNewPayload {
    /// Whether this payload is expected to fail.
    fn expects_error(&self) -> bool {
        self.error_code.is_some()
            || self.validation_error.as_ref().is_some_and(|s| !s.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Cached engine
// ---------------------------------------------------------------------------

/// The in-memory engine tree type (default mode).
type MockEngineTree = EngineApiTreeHandler<
    EthPrimitives,
    MockEthProvider,
    EthEngineTypes,
    EfXTestEngineValidator,
    MockEvmConfig,
>;

/// A cached engine for a `(fork, preHash)` pair.
///
/// In-memory `MockEthProvider` + engine tree.
struct CachedEngine {
    tree: MockEngineTree,
    /// Shared state provider -- kept alive for the engine validator's `Arc`.
    #[allow(dead_code)]
    state: Arc<Mutex<StateProviderTest>>,
    genesis_hash: B256,
}

// ---------------------------------------------------------------------------
// Engine-x test suite runner
// ---------------------------------------------------------------------------

/// Top-level handler for the engine-x test suite.
#[derive(Debug)]
pub struct EngineXTests {
    /// Path to the test fixture directory (e.g. `.../blockchain_tests_engine_x/prague/`).
    suite_path: PathBuf,
    /// Path to the pre-alloc directory (e.g. `.../blockchain_tests_engine_x/pre_alloc/`).
    pre_alloc_dir: PathBuf,
}

impl EngineXTests {
    /// Create a new engine-x test suite.
    pub fn new(suite_path: PathBuf, pre_alloc_dir: PathBuf) -> Self {
        Self { suite_path, pre_alloc_dir }
    }

    /// Walk the suite path and run all engine-x test fixtures.
    pub fn run(&self) {
        // Step 1: Load all pre-allocs in parallel.
        let pre_allocs = load_pre_allocs(&self.pre_alloc_dir);
        eprintln!("Loaded {} pre-allocs", pre_allocs.len());

        let pre_allocs = Arc::new(pre_allocs);

        let suite_path = &self.suite_path;

        // Check if the path contains subdirectories (fork dirs like eip6110_deposits/).
        let has_subdirs = WalkDir::new(suite_path)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .any(|e| e.file_type().is_dir());

        if has_subdirs {
            for entry in WalkDir::new(suite_path).min_depth(1).max_depth(1) {
                let entry = entry.expect("Failed to read directory");
                if entry.file_type().is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    self.run_dir(&name, &entry.into_path(), &pre_allocs);
                }
            }
        } else {
            let name = suite_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "engine_x_tests".to_string());
            self.run_dir(&name, suite_path, &pre_allocs);
        }
    }

    /// Load and run all test files in a directory.
    fn run_dir(
        &self,
        name: &str,
        dir: &Path,
        pre_allocs: &Arc<HashMap<String, ExPreAlloc>>,
    ) {
        assert!(dir.exists(), "Test suite path does not exist: {dir:?}");

        // Step 2: Parse all test files and flatten into individual test items.
        let json_files = find_all_json_files(dir);
        let test_items: Vec<TestItem> = json_files
            .par_iter()
            .flat_map(|path| {
                let data = match fs::read_to_string(path) {
                    Ok(d) => d,
                    Err(_) => return vec![],
                };
                let tests: BTreeMap<String, ExTestDef> = match serde_json::from_str(&data) {
                    Ok(t) => t,
                    Err(_) => return vec![],
                };
                tests
                    .into_iter()
                    .map(|(test_name, def)| TestItem {
                        name: test_name,
                        path: path.clone(),
                        def,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        eprintln!("Collected {} tests in {name}", test_items.len());
        if test_items.is_empty() {
            return;
        }

        // Step 3: Group tests by (fork, preHash) for engine caching.
        let mut groups: HashMap<String, Vec<TestItem>> = HashMap::new();
        for item in test_items {
            let key = format!("{}:{}", item.def.network_name(), item.def.pre_hash);
            groups.entry(key).or_default().push(item);
        }

        eprintln!("Unique engines needed: {}", groups.len());

        // Step 4: Run each group. Tests within a group share an engine and
        // execute sequentially; different groups run in parallel.
        let pre_allocs = pre_allocs.clone();
        let results: Vec<CaseResult> = groups
            .into_par_iter()
            .flat_map(|(key, items)| {
                run_engine_group(&key, items, &pre_allocs)
            })
            .collect();

        let (passed, failed, skipped) = categorize_results(&results);
        print_results(name, dir, &passed, &failed, &skipped);
    }
}

/// A single test item to execute.
struct TestItem {
    name: String,
    path: PathBuf,
    def: ExTestDef,
}

impl ExTestDef {
    /// Return the fork name as a string for cache keying.
    fn network_name(&self) -> &'static str {
        fork_spec_to_str(self.network)
    }
}

fn fork_spec_to_str(spec: ForkSpec) -> &'static str {
    match spec {
        ForkSpec::Frontier => "Frontier",
        ForkSpec::Homestead => "Homestead",
        ForkSpec::EIP150 => "EIP150",
        ForkSpec::EIP158 => "EIP158",
        ForkSpec::Byzantium => "Byzantium",
        ForkSpec::Constantinople => "Constantinople",
        ForkSpec::ConstantinopleFix => "ConstantinopleFix",
        ForkSpec::Istanbul => "Istanbul",
        ForkSpec::Berlin => "Berlin",
        ForkSpec::London => "London",
        ForkSpec::Merge => "Merge",
        ForkSpec::Shanghai => "Shanghai",
        ForkSpec::Cancun => "Cancun",
        ForkSpec::Prague => "Prague",
        ForkSpec::Osaka => "Osaka",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// Pre-alloc loading
// ---------------------------------------------------------------------------

/// Load all pre-alloc JSON files from the given directory in parallel.
fn load_pre_allocs(dir: &Path) -> HashMap<String, ExPreAlloc> {
    let files: Vec<(String, PathBuf)> = WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .map(|e| {
            let key = e
                .path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            (key, e.into_path())
        })
        .collect();

    files
        .par_iter()
        .filter_map(|(key, path)| {
            let data = fs::read_to_string(path).ok()?;
            let pa: ExPreAlloc = serde_json::from_str(&data).ok()?;
            Some((key.clone(), pa))
        })
        .collect()
}

/// Recursively find all `.json` files under the given path.
fn find_all_json_files(path: &Path) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .map(|e| e.into_path())
        .collect()
}

// ---------------------------------------------------------------------------
// Group execution
// ---------------------------------------------------------------------------

/// Run all tests in a group that share the same `(fork, preHash)` engine.
fn run_engine_group(
    key: &str,
    items: Vec<TestItem>,
    pre_allocs: &HashMap<String, ExPreAlloc>,
) -> Vec<CaseResult> {
    // Parse the key back to fork + preHash.
    let (fork_str, pre_hash) = key.split_once(':').unwrap_or(("", key));
    _ = fork_str; // The fork is available on each item.

    // Look up the pre-alloc.
    let pre_alloc = match pre_allocs.get(pre_hash) {
        Some(pa) => pa,
        None => {
            // All tests in this group fail because the pre-alloc is missing.
            return items
                .into_iter()
                .map(|item| CaseResult::new(
                    &item.path,
                    item.name.clone(),
                    Err(Error::Assertion(format!(
                        "Pre-alloc {pre_hash} not found"
                    ))),
                ))
                .collect();
        }
    };

    // Create the engine once for the group.
    let chain_spec = items[0].def.network.to_chain_spec();
    let engine = match create_cached_engine(chain_spec, pre_alloc) {
        Ok(e) => e,
        Err(err) => {
            let msg = format!("Failed to create engine for {key}: {err}");
            return items
                .into_iter()
                .map(|item| CaseResult::new(&item.path, item.name.clone(), Err(Error::Assertion(msg.clone()))))
                .collect();
        }
    };

    let engine = Mutex::new(engine);

    // Run tests sequentially within the group (engine state is shared).
    items
        .into_iter()
        .map(|item| {
            let result = {
                let mut eng = engine.lock().unwrap();
                let r = run_single_test(&item.name, &item.def, &mut eng.tree);
                // Reset engine to genesis after each test.
                reset_to_genesis(&mut eng);
                r
            };
            CaseResult::new(&item.path, item.name, result)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Engine creation (reuses engine_test.rs patterns)
// ---------------------------------------------------------------------------

/// Engine validator for engine-x tests. Performs real EVM execution, trusts
/// state root from the block header (same as engine_test.rs).
struct EfXTestEngineValidator {
    chain_spec: Arc<ChainSpec>,
    evm_config: EthEvmConfig,
    inner_validator: EthereumEngineValidator,
    consensus: Arc<EthBeaconConsensus<ChainSpec>>,
    state: Arc<Mutex<StateProviderTest>>,
    tree_provider: MockEthProvider,
}

impl EfXTestEngineValidator {
    fn new(
        chain_spec: Arc<ChainSpec>,
        state: Arc<Mutex<StateProviderTest>>,
        tree_provider: MockEthProvider,
    ) -> Self {
        let evm_config = EthEvmConfig::new(chain_spec.clone());
        let inner_validator = EthereumEngineValidator::new(chain_spec.clone());
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));
        Self { chain_spec, evm_config, inner_validator, consensus, state, tree_provider }
    }

    fn execute_block(
        &self,
        block: &RecoveredBlock<Block>,
    ) -> Result<ExecutedBlock, InsertBlockErrorKind> {
        let mut state_guard = self.state.lock().unwrap();
        let state_db = StateProviderDatabase::new(&*state_guard);
        let executor = self.evm_config.batch_executor(state_db);

        let output = executor
            .execute(block)
            .map_err(|e| InsertBlockErrorKind::Other(Box::new(e)))?;

        validate_block_post_execution(
            block,
            &self.chain_spec,
            &output.receipts,
            &output.requests,
            None,
        )
        .map_err(InsertBlockErrorKind::Consensus)?;

        state_guard.apply_bundle_state(&output.state);
        state_guard.insert_block_hash(block.number, block.hash());

        self.tree_provider.add_block(block.hash(), block.clone_block());
        self.tree_provider.add_header(block.hash(), block.header().clone());

        Ok(ExecutedBlock {
            recovered_block: Arc::new(block.clone()),
            execution_output: Arc::new(output),
            trie_data: DeferredTrieData::ready(ComputedTrieData::default()),
        })
    }
}

impl std::fmt::Debug for EfXTestEngineValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EfXTestEngineValidator").finish_non_exhaustive()
    }
}

impl reth_engine_primitives::PayloadValidator<EthEngineTypes> for EfXTestEngineValidator {
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

impl reth_engine_primitives::EngineApiValidator<EthEngineTypes> for EfXTestEngineValidator {
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

impl EngineValidator<EthEngineTypes> for EfXTestEngineValidator {
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
        let sealed_block = <EthereumEngineValidator as reth_engine_primitives::PayloadValidator<EthEngineTypes>>::convert_payload_to_block(
            &self.inner_validator, payload,
        ).map_err(InsertPayloadError::Payload)?;

        let block = sealed_block.clone().try_recover().map_err(|e| {
            InsertBlockError::new(
                sealed_block.clone(),
                InsertBlockErrorKind::Other(Box::new(e)),
            )
        })?;

        {
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
        unimplemented!("validate_block not used in engine-x test runner")
    }

    fn on_inserted_executed_block(&self, _block: ExecutedBlock) {
        // no-op
    }
}

impl WaitForCaches for EfXTestEngineValidator {
    fn wait_for_caches(&self) -> CacheWaitDurations {
        CacheWaitDurations { execution_cache: Duration::ZERO, sparse_trie: Duration::ZERO }
    }
}

/// Create a cached engine from a pre-alloc definition.
fn create_cached_engine(
    chain_spec: Arc<ChainSpec>,
    pre_alloc: &ExPreAlloc,
) -> Result<CachedEngine, Error> {
    use std::sync::mpsc::channel;

    // Build in-memory state provider for EVM execution.
    let mut state_provider = StateProviderTest::default();
    for (address, account) in pre_alloc.pre.iter() {
        let storage = account
            .storage
            .iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(k, v)| (B256::from(*k), *v))
            .collect();
        let reth_account = RethAccount {
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

    // Genesis block.
    let sealed_genesis_header: SealedHeader = pre_alloc.genesis.clone().into();
    let genesis_block = SealedBlock::<Block>::from_sealed_parts(
        sealed_genesis_header.clone(),
        Default::default(),
    );
    let genesis_hash = genesis_block.hash();
    state_provider.insert_block_hash(0, genesis_hash);

    // MockEthProvider for the engine tree.
    let tree_provider =
        MockEthProvider::default().with_chain_spec((*chain_spec).clone());
    tree_provider.add_block(genesis_hash, genesis_block.clone_block());
    tree_provider.add_header(genesis_hash, sealed_genesis_header.header().clone());

    // Add genesis accounts to MockEthProvider.
    for (address, account) in pre_alloc.pre.iter() {
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

    let shared_state = Arc::new(Mutex::new(state_provider));

    let engine_validator = EfXTestEngineValidator::new(
        chain_spec.clone(),
        shared_state.clone(),
        tree_provider.clone(),
    );

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
        tree_provider,
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

    Ok(CachedEngine { tree, state: shared_state, genesis_hash })
}

// ---------------------------------------------------------------------------
// Payload parsing (mirrors engine_test.rs::parse_execution_data)
// ---------------------------------------------------------------------------

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

/// Parse execution data from an engine-x payload entry.
fn parse_execution_data(entry: &ExNewPayload) -> Result<ExecutionData, Error> {
    if entry.params.is_empty() {
        return Err(Error::Assertion("engineNewPayloads params is empty".into()));
    }

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
            let v3: ExecutionPayloadV3 =
                serde_json::from_value(entry.params[0].clone()).map_err(|e| {
                    Error::Assertion(format!("Failed to deserialize ExecutionPayloadV3: {e}"))
                })?;
            ExecutionPayload::V3(v3)
        }
    };

    let versioned_hashes: Vec<B256> = if entry.params.len() > 1 {
        serde_json::from_value(entry.params[1].clone()).map_err(|e| {
            Error::Assertion(format!("Failed to deserialize versioned_hashes: {e}"))
        })?
    } else {
        vec![]
    };

    let parent_beacon_block_root: B256 = if entry.params.len() > 2 {
        serde_json::from_value(entry.params[2].clone()).map_err(|e| {
            Error::Assertion(format!(
                "Failed to deserialize parent_beacon_block_root: {e}"
            ))
        })?
    } else {
        B256::ZERO
    };

    let sidecar = if entry.new_payload_version >= 4 && entry.params.len() > 3 {
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

// ---------------------------------------------------------------------------
// Test execution
// ---------------------------------------------------------------------------

/// Execute a single engine-x test against a cached engine via the in-memory
/// engine tree path.
fn run_single_test(
    name: &str,
    def: &ExTestDef,
    tree: &mut MockEngineTree,
) -> Result<(), Error> {
    let chain_spec = def.network.to_chain_spec();
    let mut last_valid_hash: Option<B256> = None;

    for (payload_idx, entry) in def.engine_new_payloads.iter().enumerate() {
        let expects_error = entry.expects_error();

        // Parse execution data.
        let execution_data = match parse_execution_data(entry) {
            Ok(data) => data,
            Err(err) => {
                if expects_error {
                    continue;
                }
                return Err(err);
            }
        };

        // Engine API version-specific validation.
        let engine_version = to_engine_version(entry.new_payload_version)?;

        let payload_or_attrs = PayloadOrAttributes::<
            '_,
            ExecutionData,
            EthPayloadAttributes,
        >::from_execution_payload(&execution_data);

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

        // Send NewPayload through the engine tree.
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

        // Check payload status.
        if expects_error {
            if status.is_valid() {
                return Err(Error::Assertion(format!(
                    "Test case: {name}\nPayload {payload_idx}: expected error \
                     ({:?} / error_code {:?}) but got VALID status",
                    entry.validation_error, entry.error_code,
                )));
            }
            continue;
        }

        if !status.is_valid() {
            return Err(Error::Assertion(format!(
                "Test case: {name}\nPayload {payload_idx}: expected VALID but got {status:?}"
            )));
        }

        let block_hash = status
            .latest_valid_hash
            .unwrap_or(last_valid_hash.unwrap_or(B256::ZERO));
        last_valid_hash = Some(block_hash);

        // Send ForkchoiceUpdated for valid payloads.
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

    Ok(())
}

/// Reset the engine tree's chain head back to genesis so the next test
/// starts from a clean state by sending an FCU back to genesis.
fn reset_to_genesis(engine: &mut CachedEngine) {
    let fcu_state = ForkchoiceState {
        head_block_hash: engine.genesis_hash,
        safe_block_hash: engine.genesis_hash,
        finalized_block_hash: engine.genesis_hash,
    };

    let (fcu_tx, fcu_rx) = oneshot::channel();
    let fcu_msg = FromEngine::Request(
        BeaconEngineMessage::ForkchoiceUpdated {
            state: fcu_state,
            payload_attrs: None,
            tx: fcu_tx,
            version: EngineApiMessageVersion::V3,
        }
        .into(),
    );

    // Best-effort reset: ignore errors (the engine may be in a
    // broken state after a failed test, but the next test group
    // will get a fresh engine).
    if engine.tree.on_engine_message(fcu_msg).is_ok() {
        let _ = fcu_rx.blocking_recv();
    }
}
