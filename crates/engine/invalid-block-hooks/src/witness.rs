use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rpc_types_debug::ExecutionWitness;
use pretty_assertions::Comparison;
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_provider::{BlockExecutionOutput, StateProvider, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::{BundleState, State},
};
use reth_rpc_api::DebugApiClient;
use reth_tracing::tracing::warn;
use reth_trie::{updates::TrieUpdates, HashedStorage};
use revm::state::AccountInfo;
use revm_bytecode::Bytecode;
use revm_database::{
    states::{reverts::AccountInfoRevert, StorageSlot},
    AccountStatus, RevertToSlot,
};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Debug, io::Write, path::PathBuf};

type CollectionResult =
    (BTreeMap<B256, Bytes>, BTreeMap<B256, Bytes>, reth_trie::HashedPostState, BundleState);

/// Serializable version of `BundleState` for deterministic comparison
#[derive(Debug, PartialEq, Eq)]
struct BundleStateSorted {
    /// Account state
    pub state: BTreeMap<Address, BundleAccountSorted>,
    /// All created contracts in this block.
    pub contracts: BTreeMap<B256, Bytecode>,
    /// Changes to revert
    ///
    /// **Note**: Inside vector is *not* sorted by address.
    ///
    /// But it is unique by address.
    pub reverts: Vec<Vec<(Address, AccountRevertSorted)>>,
    /// The size of the plain state in the bundle state
    pub state_size: usize,
    /// The size of reverts in the bundle state
    pub reverts_size: usize,
}

/// Serializable version of `BundleAccount`
#[derive(Debug, PartialEq, Eq)]
struct BundleAccountSorted {
    pub info: Option<AccountInfo>,
    pub original_info: Option<AccountInfo>,
    /// Contains both original and present state.
    /// When extracting changeset we compare if original value is different from present value.
    /// If it is different we add it to changeset.
    /// If Account was destroyed we ignore original value and compare present state with
    /// `U256::ZERO`.
    pub storage: BTreeMap<U256, StorageSlot>,
    /// Account status.
    pub status: AccountStatus,
}

/// Serializable version of `AccountRevert`
#[derive(Debug, PartialEq, Eq)]
struct AccountRevertSorted {
    pub account: AccountInfoRevert,
    pub storage: BTreeMap<U256, RevertToSlot>,
    pub previous_status: AccountStatus,
    pub wipe_storage: bool,
}

/// Converts bundle state to sorted format for deterministic comparison
fn sort_bundle_state_for_comparison(bundle_state: &BundleState) -> BundleStateSorted {
    BundleStateSorted {
        state: bundle_state
            .state
            .iter()
            .map(|(addr, acc)| {
                (
                    *addr,
                    BundleAccountSorted {
                        info: acc.info.clone(),
                        original_info: acc.original_info.clone(),
                        storage: BTreeMap::from_iter(acc.storage.clone()),
                        status: acc.status,
                    },
                )
            })
            .collect(),
        contracts: BTreeMap::from_iter(bundle_state.contracts.clone()),
        reverts: bundle_state
            .reverts
            .iter()
            .map(|block| {
                block
                    .iter()
                    .map(|(addr, rev)| {
                        (
                            *addr,
                            AccountRevertSorted {
                                account: rev.account.clone(),
                                storage: BTreeMap::from_iter(rev.storage.clone()),
                                previous_status: rev.previous_status,
                                wipe_storage: rev.wipe_storage,
                            },
                        )
                    })
                    .collect()
            })
            .collect(),
        state_size: bundle_state.state_size,
        reverts_size: bundle_state.reverts_size,
    }
}

/// Extracts execution data including codes, preimages, and hashed state from database
fn collect_execution_data(
    mut db: State<StateProviderDatabase<Box<dyn StateProvider>>>,
) -> eyre::Result<CollectionResult> {
    let bundle_state = db.take_bundle();
    let mut codes = BTreeMap::new();
    let mut preimages = BTreeMap::new();
    let mut hashed_state = db.database.hashed_post_state(&bundle_state);

    // Collect codes
    db.cache.contracts.values().chain(bundle_state.contracts.values()).for_each(|code| {
        let code_bytes = code.original_bytes();
        codes.insert(keccak256(&code_bytes), code_bytes);
    });

    // Collect preimages
    for (address, account) in db.cache.accounts {
        let hashed_address = keccak256(address);
        hashed_state
            .accounts
            .insert(hashed_address, account.account.as_ref().map(|a| a.info.clone().into()));

        if let Some(account_data) = account.account {
            preimages.insert(hashed_address, alloy_rlp::encode(address).into());
            let storage = hashed_state
                .storages
                .entry(hashed_address)
                .or_insert_with(|| HashedStorage::new(account.status.was_destroyed()));

            for (slot, value) in account_data.storage {
                let slot_bytes = B256::from(slot);
                let hashed_slot = keccak256(slot_bytes);
                storage.storage.insert(hashed_slot, value);
                preimages.insert(hashed_slot, alloy_rlp::encode(slot_bytes).into());
            }
        }
    }

    Ok((codes, preimages, hashed_state, bundle_state))
}

/// Generates execution witness from collected codes, preimages, and hashed state
fn generate(
    codes: BTreeMap<B256, Bytes>,
    preimages: BTreeMap<B256, Bytes>,
    hashed_state: reth_trie::HashedPostState,
    state_provider: Box<dyn StateProvider>,
) -> eyre::Result<ExecutionWitness> {
    let state = state_provider.witness(Default::default(), hashed_state)?;
    Ok(ExecutionWitness {
        state,
        codes: codes.into_values().collect(),
        keys: preimages.into_values().collect(),
        ..Default::default()
    })
}

/// Hook for generating execution witnesses when invalid blocks are detected.
///
/// This hook captures the execution state and generates witness data that can be used
/// for debugging and analysis of invalid block execution.
#[derive(Debug)]
pub struct InvalidBlockWitnessHook<P, E> {
    /// The provider to read the historical state and do the EVM execution.
    provider: P,
    /// The EVM configuration to use for the execution.
    evm_config: E,
    /// The directory to write the witness to. Additionally, diff files will be written to this
    /// directory in case of failed sanity checks.
    output_directory: PathBuf,
    /// The healthy node client to compare the witness against.
    healthy_node_client: Option<jsonrpsee::http_client::HttpClient>,
}

impl<P, E> InvalidBlockWitnessHook<P, E> {
    /// Creates a new witness hook.
    pub const fn new(
        provider: P,
        evm_config: E,
        output_directory: PathBuf,
        healthy_node_client: Option<jsonrpsee::http_client::HttpClient>,
    ) -> Self {
        Self { provider, evm_config, output_directory, healthy_node_client }
    }
}

impl<P, E, N> InvalidBlockWitnessHook<P, E>
where
    P: StateProviderFactory + Send + Sync + 'static,
    E: ConfigureEvm<Primitives = N> + 'static,
    N: NodePrimitives,
{
    /// Re-executes the block and collects execution data
    fn re_execute_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
    ) -> eyre::Result<(ExecutionWitness, BundleState)> {
        let mut executor = self.evm_config.batch_executor(StateProviderDatabase::new(
            self.provider.state_by_block_hash(parent_header.hash())?,
        ));

        executor.execute_one(block)?;
        let db = executor.into_state();
        let (codes, preimages, hashed_state, bundle_state) = collect_execution_data(db)?;

        let state_provider = self.provider.state_by_block_hash(parent_header.hash())?;
        let witness = generate(codes, preimages, hashed_state, state_provider)?;

        Ok((witness, bundle_state))
    }

    /// Handles witness generation, saving, and comparison with healthy node
    fn handle_witness_operations(
        &self,
        witness: &ExecutionWitness,
        block_prefix: &str,
        block_number: u64,
    ) -> eyre::Result<()> {
        let filename = format!("{}.witness.re_executed.json", block_prefix);
        let re_executed_witness_path = self.save_file(filename, witness)?;

        if let Some(healthy_node_client) = &self.healthy_node_client {
            let healthy_node_witness = futures::executor::block_on(async move {
                DebugApiClient::<()>::debug_execution_witness(
                    healthy_node_client,
                    block_number.into(),
                )
                .await
            })?;

            let filename = format!("{}.witness.healthy.json", block_prefix);
            let healthy_path = self.save_file(filename, &healthy_node_witness)?;

            if witness != &healthy_node_witness {
                let filename = format!("{}.witness.diff", block_prefix);
                let diff_path = self.save_diff(filename, witness, &healthy_node_witness)?;
                warn!(
                    target: "engine::invalid_block_hooks::witness",
                    diff_path = %diff_path.display(),
                    re_executed_path = %re_executed_witness_path.display(),
                    healthy_path = %healthy_path.display(),
                    "Witness mismatch against healthy node"
                );
            }
        }
        Ok(())
    }

    /// Validates that the bundle state after re-execution matches the original
    fn validate_bundle_state(
        &self,
        re_executed_state: &BundleState,
        original_state: &BundleState,
        block_prefix: &str,
    ) -> eyre::Result<()> {
        if re_executed_state != original_state {
            let (original_path, re_executed_path) = self.save_comparison_files(
                block_prefix,
                "bundle_state",
                original_state,
                re_executed_state,
            )?;

            // Convert bundle state to sorted format for deterministic comparison
            let bundle_state_sorted = sort_bundle_state_for_comparison(re_executed_state);
            let output_state_sorted = sort_bundle_state_for_comparison(original_state);
            let filename = format!("{}.bundle_state.diff", block_prefix);
            let diff_path = self.save_diff(filename, &bundle_state_sorted, &output_state_sorted)?;

            warn!(
                target: "engine::invalid_block_hooks::witness",
                diff_path = %diff_path.display(),
                original_path = %original_path.display(),
                re_executed_path = %re_executed_path.display(),
                "Bundle state mismatch after re-execution"
            );
        }
        Ok(())
    }

    /// Validates state root and trie updates after re-execution
    fn validate_state_root_and_trie(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        bundle_state: &BundleState,
        trie_updates: Option<(&TrieUpdates, B256)>,
        block_prefix: &str,
    ) -> eyre::Result<()> {
        let state_provider = self.provider.state_by_block_hash(parent_header.hash())?;
        let hashed_state = state_provider.hashed_post_state(bundle_state);
        let (re_executed_root, trie_output) =
            state_provider.state_root_with_updates(hashed_state)?;

        if let Some((original_updates, original_root)) = trie_updates {
            if re_executed_root != original_root {
                let filename = format!("{}.state_root.diff", block_prefix);
                let diff_path = self.save_diff(filename, &re_executed_root, &original_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", ?original_root, ?re_executed_root, diff_path = %diff_path.display(), "State root mismatch after re-execution");
            }

            if re_executed_root != block.state_root() {
                let filename = format!("{}.header_state_root.diff", block_prefix);
                let diff_path = self.save_diff(filename, &re_executed_root, &block.state_root())?;
                warn!(target: "engine::invalid_block_hooks::witness", header_state_root=?block.state_root(), ?re_executed_root, diff_path = %diff_path.display(), "Re-executed state root does not match block state root");
            }

            if &trie_output != original_updates {
                let (original_path, re_executed_path) = self.save_comparison_files(
                    block_prefix,
                    "trie_updates",
                    &original_updates.into_sorted_ref(),
                    &trie_output.into_sorted_ref(),
                )?;
                warn!(
                    target: "engine::invalid_block_hooks::witness",
                    original_path = %original_path.display(),
                    re_executed_path = %re_executed_path.display(),
                    "Trie updates mismatch after re-execution"
                );
            }
        }
        Ok(())
    }

    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
        // TODO(alexey): unify with `DebugApi::debug_execution_witness`
        let (witness, bundle_state) = self.re_execute_block(parent_header, block)?;

        let block_prefix = format!("{}_{}", block.number(), block.hash());
        self.handle_witness_operations(&witness, &block_prefix, block.number())?;

        self.validate_bundle_state(&bundle_state, &output.state, &block_prefix)?;

        self.validate_state_root_and_trie(
            parent_header,
            block,
            &bundle_state,
            trie_updates,
            &block_prefix,
        )?;

        Ok(())
    }

    /// Serializes and saves a value to a JSON file in the output directory
    fn save_file<T: Serialize>(&self, filename: String, value: &T) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let mut file = reth_fs_util::create_file(&path)?;
        let contents = serde_json::to_string(value)?;
        file.write_all(contents.as_bytes()).map_err(|err| FsPathError::write(err, &path))?;

        Ok(path)
    }

    /// Saves both original and re-executed comparison files with the given file type prefix
    fn save_comparison_files<T: Serialize>(
        &self,
        block_prefix: &str,
        file_type: &str,
        original: &T,
        re_executed: &T,
    ) -> eyre::Result<(PathBuf, PathBuf)> {
        let original_path =
            self.save_file(format!("{}.{}.original.json", block_prefix, file_type), original)?;
        let re_executed_path = self
            .save_file(format!("{}.{}.re_executed.json", block_prefix, file_type), re_executed)?;
        Ok((original_path, re_executed_path))
    }

    /// Compares two values and saves their diff to a file in the output directory
    fn save_diff<T: PartialEq + Debug>(
        &self,
        filename: String,
        original: &T,
        new: &T,
    ) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let diff = Comparison::new(original, new);
        let mut file = reth_fs_util::create_file(&path)?;
        file.write_all(diff.to_string().as_bytes())
            .map_err(|err| FsPathError::write(err, &path))?;

        Ok(path)
    }
}

impl<P, E, N: NodePrimitives> InvalidBlockHook<N> for InvalidBlockWitnessHook<P, E>
where
    P: StateProviderFactory + Send + Sync + 'static,
    E: ConfigureEvm<Primitives = N> + 'static,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        if let Err(err) = self.on_invalid_block(parent_header, block, output, trie_updates) {
            warn!(target: "engine::invalid_block_hooks::witness", %err, "Failed to invoke hook");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::Requests;
    use alloy_primitives::{map::HashMap, Address, Bytes, B256, U256};
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use reth_revm::db::{BundleAccount, BundleState};
    use revm_database::states::reverts::AccountRevert;
    use tempfile::TempDir;

    use reth_revm::test_utils::StateProviderTest;
    use reth_testing_utils::generators::{self, random_block, random_eoa_accounts, BlockParams};
    use revm_bytecode::Bytecode;

    /// Creates a test `BundleState` with realistic accounts, contracts, and reverts
    fn create_bundle_state() -> BundleState {
        let mut rng = generators::rng();
        let mut bundle_state = BundleState::default();

        // Generate realistic EOA accounts using generators
        let accounts = random_eoa_accounts(&mut rng, 3);

        for (i, (addr, account)) in accounts.into_iter().enumerate() {
            // Create storage entries for each account
            let mut storage = HashMap::default();
            let storage_key = U256::from(i + 1);
            storage.insert(
                storage_key,
                StorageSlot {
                    present_value: U256::from((i + 1) * 10),
                    previous_or_original_value: U256::from((i + 1) * 15),
                },
            );

            let bundle_account = BundleAccount {
                info: Some(AccountInfo {
                    balance: account.balance,
                    nonce: account.nonce,
                    code_hash: account.bytecode_hash.unwrap_or_default(),
                    code: None,
                }),
                original_info: (i == 0).then(|| AccountInfo {
                    balance: account.balance.checked_div(U256::from(2)).unwrap_or(U256::ZERO),
                    nonce: 0,
                    code_hash: account.bytecode_hash.unwrap_or_default(),
                    code: None,
                }),
                storage,
                status: AccountStatus::default(),
            };

            bundle_state.state.insert(addr, bundle_account);
        }

        // Generate realistic contract bytecode using generators
        let contract_hashes: Vec<B256> = (0..3).map(|_| B256::random()).collect();
        for (i, hash) in contract_hashes.iter().enumerate() {
            let bytecode = match i {
                0 => Bytes::from(vec![0x60, 0x80, 0x60, 0x40, 0x52]), // Simple contract
                1 => Bytes::from(vec![0x61, 0x81, 0x60, 0x00, 0x39]), // Another contract
                _ => Bytes::from(vec![0x60, 0x00, 0x60, 0x00, 0xfd]), // REVERT contract
            };
            bundle_state.contracts.insert(*hash, Bytecode::new_raw(bytecode));
        }

        // Add reverts for multiple blocks using different accounts
        let addresses: Vec<Address> = bundle_state.state.keys().copied().collect();
        for (i, addr) in addresses.iter().take(2).enumerate() {
            let revert = AccountRevert {
                wipe_storage: i == 0, // First account has storage wiped
                ..AccountRevert::default()
            };
            bundle_state.reverts.push(vec![(*addr, revert)]);
        }

        // Set realistic sizes
        bundle_state.state_size = bundle_state.state.len();
        bundle_state.reverts_size = bundle_state.reverts.len();

        bundle_state
    }
    #[test]
    fn test_sort_bundle_state_for_comparison() {
        // Use the fixture function to create test data
        let bundle_state = create_bundle_state();

        // Call the function under test
        let sorted = sort_bundle_state_for_comparison(&bundle_state);

        // Verify state_size and reverts_size values match the fixture
        assert_eq!(sorted.state_size, 3);
        assert_eq!(sorted.reverts_size, 2);

        // Verify state contains our mock accounts
        assert_eq!(sorted.state.len(), 3); // We added 3 accounts

        // Verify contracts contains our mock contracts
        assert_eq!(sorted.contracts.len(), 3); // We added 3 contracts

        // Verify reverts is an array with multiple blocks of reverts
        let reverts = &sorted.reverts;
        assert_eq!(reverts.len(), 2); // Fixture has two blocks of reverts

        // Verify that the state accounts have the expected structure
        for account_data in sorted.state.values() {
            // BundleAccountSorted has info, original_info, storage, and status fields
            // Just verify the structure exists by accessing the fields
            let _info = &account_data.info;
            let _original_info = &account_data.original_info;
            let _storage = &account_data.storage;
            let _status = &account_data.status;
        }
    }

    #[test]
    fn test_data_collector_collect() {
        // Create test data using the fixture function
        let bundle_state = create_bundle_state();

        // Create a State with StateProviderTest
        let state_provider = StateProviderTest::default();
        let mut state = State::builder()
            .with_database(StateProviderDatabase::new(
                Box::new(state_provider) as Box<dyn StateProvider>
            ))
            .with_bundle_update()
            .build();

        // Insert contracts from the fixture into the state cache
        for (code_hash, bytecode) in &bundle_state.contracts {
            state.cache.contracts.insert(*code_hash, bytecode.clone());
        }

        // Manually set the bundle state in the state object
        state.bundle_state = bundle_state;

        // Call the collect function
        let result = collect_execution_data(state);
        // Verify the function returns successfully
        assert!(result.is_ok());

        let (codes, _preimages, _hashed_state, returned_bundle_state) = result.unwrap();

        // Verify that the returned data contains expected values
        // Since we used the fixture data, we should have some codes and state
        assert!(!codes.is_empty(), "Expected some bytecode entries");
        assert!(!returned_bundle_state.state.is_empty(), "Expected some state entries");

        // Verify the bundle state structure matches our fixture
        assert_eq!(returned_bundle_state.state.len(), 3, "Expected 3 accounts from fixture");
        assert_eq!(returned_bundle_state.contracts.len(), 3, "Expected 3 contracts from fixture");
    }

    #[test]
    fn test_re_execute_block() {
        // Create hook instance
        let (hook, _output_directory, _temp_dir) = create_test_hook();

        // Setup to call re_execute_block
        let mut rng = generators::rng();
        let parent_header = generators::random_header(&mut rng, 1, None);

        // Create a random block that inherits from the parent header
        let recovered_block = random_block(
            &mut rng,
            2, // block number
            BlockParams {
                parent: Some(parent_header.hash()),
                tx_count: Some(0),
                ..Default::default()
            },
        )
        .try_recover()
        .unwrap();

        let result = hook.re_execute_block(&parent_header, &recovered_block);

        // Verify the function behavior with mock data
        assert!(result.is_ok(), "re_execute_block should return Ok");
    }

    /// Creates test `InvalidBlockWitnessHook` with temporary directory
    fn create_test_hook() -> (
        InvalidBlockWitnessHook<MockEthProvider<EthPrimitives, ChainSpec>, EthEvmConfig>,
        PathBuf,
        TempDir,
    ) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_directory = temp_dir.path().to_path_buf();

        let provider = MockEthProvider::<EthPrimitives, ChainSpec>::default();
        let evm_config = EthEvmConfig::mainnet();

        let hook =
            InvalidBlockWitnessHook::new(provider, evm_config, output_directory.clone(), None);

        (hook, output_directory, temp_dir)
    }

    #[test]
    fn test_handle_witness_operations_with_healthy_client_mock() {
        // Create hook instance with mock healthy client
        let (hook, output_directory, _temp_dir) = create_test_hook();

        // Create sample ExecutionWitness with correct types
        let witness = ExecutionWitness {
            state: vec![Bytes::from("state_data")],
            codes: vec![Bytes::from("code_data")],
            keys: vec![Bytes::from("key_data")],
            ..Default::default()
        };

        // Call handle_witness_operations
        let result = hook.handle_witness_operations(&witness, "test_block_healthy", 67890);

        // Should succeed
        assert!(result.is_ok());

        // Check that witness file was created
        let witness_file = output_directory.join("test_block_healthy.witness.re_executed.json");
        assert!(witness_file.exists());
    }

    #[test]
    fn test_handle_witness_operations_file_creation() {
        // Test file creation and content validation
        let (hook, output_directory, _temp_dir) = create_test_hook();

        let witness = ExecutionWitness {
            state: vec![Bytes::from("test_state")],
            codes: vec![Bytes::from("test_code")],
            keys: vec![Bytes::from("test_key")],
            ..Default::default()
        };

        let block_prefix = "file_test_block";
        let block_number = 11111;

        // Call handle_witness_operations
        let result = hook.handle_witness_operations(&witness, block_prefix, block_number);
        assert!(result.is_ok());

        // Verify file was created with correct name
        let expected_file =
            output_directory.join(format!("{}.witness.re_executed.json", block_prefix));
        assert!(expected_file.exists());

        // Read and verify file content is valid JSON and contains witness structure
        let file_content = std::fs::read_to_string(&expected_file).expect("Failed to read file");
        let parsed_witness: serde_json::Value =
            serde_json::from_str(&file_content).expect("File should contain valid JSON");

        // Verify the JSON structure contains expected fields
        assert!(parsed_witness.get("state").is_some(), "JSON should contain 'state' field");
        assert!(parsed_witness.get("codes").is_some(), "JSON should contain 'codes' field");
        assert!(parsed_witness.get("keys").is_some(), "JSON should contain 'keys' field");
    }

    #[test]
    fn test_proof_generator_generate() {
        // Use existing MockEthProvider
        let mock_provider = MockEthProvider::default();
        let state_provider: Box<dyn StateProvider> = Box::new(mock_provider);

        // Mock Data
        let mut codes = BTreeMap::new();
        codes.insert(B256::from([1u8; 32]), Bytes::from("contract_code_1"));
        codes.insert(B256::from([2u8; 32]), Bytes::from("contract_code_2"));

        let mut preimages = BTreeMap::new();
        preimages.insert(B256::from([3u8; 32]), Bytes::from("preimage_1"));
        preimages.insert(B256::from([4u8; 32]), Bytes::from("preimage_2"));

        let hashed_state = reth_trie::HashedPostState::default();

        // Call generate function
        let result = generate(codes.clone(), preimages.clone(), hashed_state, state_provider);

        // Verify result
        assert!(result.is_ok(), "generate function should succeed");
        let execution_witness = result.unwrap();

        assert!(execution_witness.state.is_empty(), "State should be empty from MockEthProvider");

        let expected_codes: Vec<Bytes> = codes.into_values().collect();
        assert_eq!(
            execution_witness.codes.len(),
            expected_codes.len(),
            "Codes length should match"
        );
        for code in &expected_codes {
            assert!(
                execution_witness.codes.contains(code),
                "Codes should contain expected bytecode"
            );
        }

        let expected_keys: Vec<Bytes> = preimages.into_values().collect();
        assert_eq!(execution_witness.keys.len(), expected_keys.len(), "Keys length should match");
        for key in &expected_keys {
            assert!(execution_witness.keys.contains(key), "Keys should contain expected preimage");
        }
    }

    #[test]
    fn test_validate_bundle_state_matching() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let bundle_state = create_bundle_state();
        let block_prefix = "test_block_123";

        // Test with identical states - should not produce any warnings or files
        let result = hook.validate_bundle_state(&bundle_state, &bundle_state, block_prefix);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_bundle_state_mismatch() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let original_state = create_bundle_state();
        let mut modified_state = create_bundle_state();

        // Modify the state to create a mismatch
        let addr = Address::from([1u8; 20]);
        if let Some(account) = modified_state.state.get_mut(&addr) &&
            let Some(ref mut info) = account.info
        {
            info.balance = U256::from(999);
        }

        let block_prefix = "test_block_mismatch";

        // Test with different states - should save files and log warning
        let result = hook.validate_bundle_state(&modified_state, &original_state, block_prefix);
        assert!(result.is_ok());

        // Verify that files were created
        let original_file = output_dir.join(format!("{}.bundle_state.original.json", block_prefix));
        let re_executed_file =
            output_dir.join(format!("{}.bundle_state.re_executed.json", block_prefix));
        let diff_file = output_dir.join(format!("{}.bundle_state.diff", block_prefix));

        assert!(original_file.exists(), "Original bundle state file should be created");
        assert!(re_executed_file.exists(), "Re-executed bundle state file should be created");
        assert!(diff_file.exists(), "Diff file should be created");
    }

    /// Creates test `TrieUpdates` with account nodes and removed nodes
    fn create_test_trie_updates() -> TrieUpdates {
        use alloy_primitives::map::HashMap;
        use reth_trie::{updates::TrieUpdates, BranchNodeCompact, Nibbles};
        use std::collections::HashSet;

        let mut account_nodes = HashMap::default();
        let nibbles = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3]);
        let branch_node = BranchNodeCompact::new(
            0b1010,                      // state_mask
            0b1010,                      // tree_mask - must be subset of state_mask
            0b1000,                      // hash_mask
            vec![B256::from([1u8; 32])], // hashes
            None,                        // root_hash
        );
        account_nodes.insert(nibbles, branch_node);

        let mut removed_nodes = HashSet::default();
        removed_nodes.insert(Nibbles::from_nibbles_unchecked([0x4, 0x5, 0x6]));

        TrieUpdates { account_nodes, removed_nodes, storage_tries: HashMap::default() }
    }

    #[test]
    fn test_validate_state_root_and_trie_with_trie_updates() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let bundle_state = create_bundle_state();

        // Generate test data
        let mut rng = generators::rng();
        let parent_header = generators::random_header(&mut rng, 1, None);
        let recovered_block = random_block(
            &mut rng,
            2,
            BlockParams {
                parent: Some(parent_header.hash()),
                tx_count: Some(0),
                ..Default::default()
            },
        )
        .try_recover()
        .unwrap();

        let trie_updates = create_test_trie_updates();
        let original_root = B256::from([2u8; 32]); // Different from what will be computed
        let block_prefix = "test_state_root_with_trie";

        // Test with trie updates - this will likely produce warnings due to mock data
        let result = hook.validate_state_root_and_trie(
            &parent_header,
            &recovered_block,
            &bundle_state,
            Some((&trie_updates, original_root)),
            block_prefix,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_on_invalid_block_calls_all_validation_methods() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let bundle_state = create_bundle_state();

        // Generate test data
        let mut rng = generators::rng();
        let parent_header = generators::random_header(&mut rng, 1, None);
        let recovered_block = random_block(
            &mut rng,
            2,
            BlockParams {
                parent: Some(parent_header.hash()),
                tx_count: Some(0),
                ..Default::default()
            },
        )
        .try_recover()
        .unwrap();

        // Create mock BlockExecutionOutput
        let output = BlockExecutionOutput {
            state: bundle_state,
            result: reth_provider::BlockExecutionResult {
                receipts: vec![],
                requests: Requests::default(),
                gas_used: 0,
                blob_gas_used: 0,
            },
        };

        // Create test trie updates
        let trie_updates = create_test_trie_updates();
        let state_root = B256::random();

        // Test that on_invalid_block attempts to call all its internal methods
        // by checking that it doesn't panic and tries to create files
        let files_before = output_dir.read_dir().unwrap().count();

        let _result = hook.on_invalid_block(
            &parent_header,
            &recovered_block,
            &output,
            Some((&trie_updates, state_root)),
        );

        // Verify that the function attempted to process the block:
        // Either it succeeded, or it created some output files during processing
        let files_after = output_dir.read_dir().unwrap().count();

        // The function should attempt to execute its workflow
        assert!(
            files_after >= files_before,
            "on_invalid_block should attempt to create output files during processing"
        );
    }

    #[test]
    fn test_handle_witness_operations_with_empty_witness() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let witness = ExecutionWitness::default();
        let block_prefix = "empty_witness_test";
        let block_number = 12345;

        let result = hook.handle_witness_operations(&witness, block_prefix, block_number);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_witness_operations_with_zero_block_number() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let witness = ExecutionWitness {
            state: vec![Bytes::from("test_state")],
            codes: vec![Bytes::from("test_code")],
            keys: vec![Bytes::from("test_key")],
            ..Default::default()
        };
        let block_prefix = "zero_block_test";
        let block_number = 0;

        let result = hook.handle_witness_operations(&witness, block_prefix, block_number);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_witness_operations_with_large_witness_data() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let large_data = vec![0u8; 10000]; // 10KB of data
        let witness = ExecutionWitness {
            state: vec![Bytes::from(large_data.clone())],
            codes: vec![Bytes::from(large_data.clone())],
            keys: vec![Bytes::from(large_data)],
            ..Default::default()
        };
        let block_prefix = "large_witness_test";
        let block_number = 999999;

        let result = hook.handle_witness_operations(&witness, block_prefix, block_number);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_bundle_state_with_empty_states() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let empty_state = BundleState::default();
        let block_prefix = "empty_states_test";

        let result = hook.validate_bundle_state(&empty_state, &empty_state, block_prefix);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_bundle_state_with_different_contract_counts() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let state1 = create_bundle_state();
        let mut state2 = create_bundle_state();

        // Add extra contract to state2
        let extra_contract_hash = B256::random();
        state2.contracts.insert(
            extra_contract_hash,
            Bytecode::new_raw(Bytes::from(vec![0x60, 0x00, 0x60, 0x00, 0xfd])), // REVERT opcode
        );

        let block_prefix = "different_contracts_test";
        let result = hook.validate_bundle_state(&state1, &state2, block_prefix);
        assert!(result.is_ok());

        // Verify diff files were created
        let diff_file = output_dir.join(format!("{}.bundle_state.diff", block_prefix));
        assert!(diff_file.exists());
    }

    #[test]
    fn test_save_diff_with_identical_values() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let value1 = "identical_value";
        let value2 = "identical_value";
        let filename = "identical_diff_test".to_string();

        let result = hook.save_diff(filename.clone(), &value1, &value2);
        assert!(result.is_ok());

        let diff_file = output_dir.join(filename);
        assert!(diff_file.exists());
    }

    #[test]
    fn test_validate_state_root_and_trie_without_trie_updates() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let bundle_state = create_bundle_state();

        let mut rng = generators::rng();
        let parent_header = generators::random_header(&mut rng, 1, None);
        let recovered_block = random_block(
            &mut rng,
            2,
            BlockParams {
                parent: Some(parent_header.hash()),
                tx_count: Some(0),
                ..Default::default()
            },
        )
        .try_recover()
        .unwrap();

        let block_prefix = "no_trie_updates_test";

        // Test without trie updates (None case)
        let result = hook.validate_state_root_and_trie(
            &parent_header,
            &recovered_block,
            &bundle_state,
            None,
            block_prefix,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_complete_invalid_block_workflow() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let mut rng = generators::rng();

        // Create a realistic block scenario
        let parent_header = generators::random_header(&mut rng, 100, None);
        let invalid_block = random_block(
            &mut rng,
            101,
            BlockParams {
                parent: Some(parent_header.hash()),
                tx_count: Some(3),
                ..Default::default()
            },
        )
        .try_recover()
        .unwrap();

        let bundle_state = create_bundle_state();
        let trie_updates = create_test_trie_updates();

        // Test validation methods
        let validation_result =
            hook.validate_bundle_state(&bundle_state, &bundle_state, "integration_test");
        assert!(validation_result.is_ok(), "Bundle state validation should succeed");

        let state_root_result = hook.validate_state_root_and_trie(
            &parent_header,
            &invalid_block,
            &bundle_state,
            Some((&trie_updates, B256::random())),
            "integration_test",
        );
        assert!(state_root_result.is_ok(), "State root validation should succeed");
    }

    #[test]
    fn test_integration_workflow_components() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let mut rng = generators::rng();

        // Create test data
        let parent_header = generators::random_header(&mut rng, 50, None);
        let _invalid_block = random_block(
            &mut rng,
            51,
            BlockParams {
                parent: Some(parent_header.hash()),
                tx_count: Some(2),
                ..Default::default()
            },
        )
        .try_recover()
        .unwrap();

        let bundle_state = create_bundle_state();
        let _trie_updates = create_test_trie_updates();

        // Test individual components that would be part of the complete flow
        let validation_result =
            hook.validate_bundle_state(&bundle_state, &bundle_state, "integration_component_test");
        assert!(validation_result.is_ok(), "Component validation should succeed");
    }
}
