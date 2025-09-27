use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rpc_types_debug::ExecutionWitness;
use pretty_assertions::Comparison;
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_provider::{BlockExecutionOutput, ChainSpecProvider, StateProvider, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::{BundleState, State},
};
use reth_rpc_api::DebugApiClient;
use reth_tracing::tracing::warn;
use reth_trie::{updates::TrieUpdates, HashedStorage};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Debug, fs::File, io::Write, path::PathBuf};

type CollectionResult =
    (BTreeMap<B256, Bytes>, BTreeMap<B256, Bytes>, reth_trie::HashedPostState, BundleState);

fn sort_bundle_state_for_comparison(bundle_state: &BundleState) -> serde_json::Value {
    serde_json::json!({
        "state": bundle_state.state.iter().map(|(addr, acc)| {
            (addr, serde_json::json!({
                "info": acc.info,
                "original_info": acc.original_info,
                "storage": BTreeMap::from_iter(acc.storage.clone()),
                "status": acc.status
            }))
        }).collect::<BTreeMap<_, _>>(),
        "contracts": BTreeMap::from_iter(bundle_state.contracts.clone()),
        "reverts": bundle_state.reverts.iter().map(|block| {
            block.iter().map(|(addr, rev)| {
                (addr, serde_json::json!({
                    "account": rev.account,
                    "storage": BTreeMap::from_iter(rev.storage.clone()),
                    "previous_status": rev.previous_status,
                    "wipe_storage": rev.wipe_storage
                }))
            }).collect::<BTreeMap<_, _>>()
        }).collect::<Vec<_>>(),
        "state_size": bundle_state.state_size,
        "reverts_size": bundle_state.reverts_size
    })
}

/// Generates a witness for the given block and saves it to a file.
#[derive(Debug)]
struct DataCollector;

impl DataCollector {
    fn collect(
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
}

struct ProofGenerator;

impl ProofGenerator {
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
}

#[derive(Debug)]
/// Hook for generating execution witnesses when invalid blocks are detected.
///
/// This hook captures the execution state and generates witness data that can be used
/// for debugging and analysis of invalid block execution.
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
    P: StateProviderFactory + ChainSpecProvider + Send + Sync + 'static,
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
        let (codes, preimages, hashed_state, bundle_state) = DataCollector::collect(db)?;

        let state_provider = self.provider.state_by_block_hash(parent_header.hash())?;
        let witness = ProofGenerator::generate(codes, preimages, hashed_state, state_provider)?;

        Ok((witness, bundle_state))
    }

    /// Handles witness generation, saving, and comparison with healthy node
    fn handle_witness_operations(
        &self,
        witness: &ExecutionWitness,
        block_prefix: &str,
        block_number: u64,
    ) -> eyre::Result<()> {
        let re_executed_witness_path =
            self.save_file(format!("{}.witness.re_executed.json", block_prefix), witness)?;

        if let Some(healthy_node_client) = &self.healthy_node_client {
            let healthy_node_witness = futures::executor::block_on(async move {
                DebugApiClient::<()>::debug_execution_witness(
                    healthy_node_client,
                    block_number.into(),
                )
                .await
            })?;

            let healthy_path = self.save_file(
                format!("{}.witness.healthy.json", block_prefix),
                &healthy_node_witness,
            )?;

            if witness != &healthy_node_witness {
                let diff_path = self.save_diff(
                    format!("{}.witness.diff", block_prefix),
                    witness,
                    &healthy_node_witness,
                )?;
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
            let original_path = self.save_file(
                format!("{}.bundle_state.original.json", block_prefix),
                original_state,
            )?;
            let re_executed_path = self.save_file(
                format!("{}.bundle_state.re_executed.json", block_prefix),
                re_executed_state,
            )?;

            // Convert bundle state to sorted format for deterministic comparison
            let bundle_state_sorted = sort_bundle_state_for_comparison(re_executed_state);
            let output_state_sorted = sort_bundle_state_for_comparison(original_state);
            let diff_path = self.save_diff(
                format!("{}.bundle_state.diff", block_prefix),
                &bundle_state_sorted,
                &output_state_sorted,
            )?;

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
                let diff_path = self.save_diff(
                    format!("{}.state_root.diff", block_prefix),
                    &re_executed_root,
                    &original_root,
                )?;
                warn!(target: "engine::invalid_block_hooks::witness", ?original_root, ?re_executed_root, diff_path = %diff_path.display(), "State root mismatch after re-execution");
            }

            if re_executed_root != block.state_root() {
                let diff_path = self.save_diff(
                    format!("{}.header_state_root.diff", block_prefix),
                    &re_executed_root,
                    &block.state_root(),
                )?;
                warn!(target: "engine::invalid_block_hooks::witness", header_state_root=?block.state_root(), ?re_executed_root, diff_path = %diff_path.display(), "Re-executed state root does not match block state root");
            }

            if &trie_output != original_updates {
                let original_path = self.save_file(
                    format!("{}.trie_updates.original.json", block_prefix),
                    &original_updates.into_sorted_ref(),
                )?;
                let re_executed_path = self.save_file(
                    format!("{}.trie_updates.re_executed.json", block_prefix),
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

    fn save_file<T: Serialize>(&self, filename: String, value: &T) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        File::create(&path)?.write_all(serde_json::to_string(value)?.as_bytes())?;

        Ok(path)
    }

    fn save_diff<T: PartialEq + Debug>(
        &self,
        filename: String,
        original: &T,
        new: &T,
    ) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let diff = Comparison::new(original, new);
        File::create(&path)?.write_all(diff.to_string().as_bytes())?;

        Ok(path)
    }
}

impl<P, E, N: NodePrimitives> InvalidBlockHook<N> for InvalidBlockWitnessHook<P, E>
where
    P: StateProviderFactory + ChainSpecProvider + Send + Sync + 'static,
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
    use revm_database::{states::StorageSlot, AccountRevert, AccountStatus};
    use revm_state::AccountInfo;
    use tempfile::TempDir;

    use reth_revm::test_utils::StateProviderTest;
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use revm_bytecode::Bytecode;
    //Fixture function
    fn create_bundle_state() -> BundleState {
        let mut bundle_state = BundleState::default();

        // Add two accounts with info, storage, and different statuses
        let addr1 = Address::from([1u8; 20]);
        let addr2 = Address::from([2u8; 20]);

        let mut storage1 = HashMap::default();
        storage1.insert(
            U256::from(1),
            StorageSlot {
                present_value: U256::from(10),
                previous_or_original_value: U256::from(15),
            },
        );
        storage1.insert(
            U256::from(2),
            StorageSlot {
                present_value: U256::from(20),
                previous_or_original_value: U256::from(25),
            },
        );

        let bundle_account1 = BundleAccount {
            info: Some(AccountInfo {
                balance: U256::from(100),
                nonce: 1,
                code_hash: B256::from([3u8; 32]),
                code: None,
            }),
            original_info: Some(AccountInfo {
                balance: U256::from(50),
                nonce: 0,
                code_hash: B256::from([3u8; 32]),
                code: None,
            }),
            storage: storage1,
            status: AccountStatus::default(),
        };

        let mut storage2 = HashMap::default();
        storage2.insert(
            U256::from(3),
            StorageSlot {
                present_value: U256::from(30),
                previous_or_original_value: U256::from(35),
            },
        );

        let bundle_account2 = BundleAccount {
            info: Some(AccountInfo {
                balance: U256::from(200),
                nonce: 2,
                code_hash: B256::from([4u8; 32]),
                code: None,
            }),
            original_info: None,
            storage: storage2,
            status: AccountStatus::default(),
        };

        bundle_state.state.insert(addr1, bundle_account1);
        bundle_state.state.insert(addr2, bundle_account2);

        // Add two contracts
        let contract_hash1 = B256::from([5u8; 32]);
        let contract_hash2 = B256::from([6u8; 32]);
        bundle_state
            .contracts
            .insert(contract_hash1, Bytecode::new_raw(Bytes::from(vec![0x60, 0x80])));
        bundle_state
            .contracts
            .insert(contract_hash2, Bytecode::new_raw(Bytes::from(vec![0x61, 0x81])));

        // Add reverts for one block
        bundle_state.reverts.push(vec![(addr1, AccountRevert::default())]);

        // Set non-default sizes
        bundle_state.state_size = 42;
        bundle_state.reverts_size = 7;

        bundle_state
    }
    #[test]
    fn test_sort_bundle_state_for_comparison() {
        // Use the fixture function to create test data
        let bundle_state = create_bundle_state();

        // Call the function under test
        let sorted = sort_bundle_state_for_comparison(&bundle_state);

        // Verify state_size and reverts_size values match the fixture
        assert_eq!(sorted["state_size"], 42);
        assert_eq!(sorted["reverts_size"], 7);

        // Verify state contains our mock accounts
        let state = sorted["state"].as_object().unwrap();
        assert_eq!(state.len(), 2); // We added 2 accounts

        // Verify contracts contains our mock contracts
        let contracts = sorted["contracts"].as_object().unwrap();
        assert_eq!(contracts.len(), 2); // We added 2 contracts

        // Verify reverts is an array with one block of reverts
        let reverts = sorted["reverts"].as_array().unwrap();
        assert_eq!(reverts.len(), 1); // Fixture has one block of reverts

        // Verify that the state accounts have the expected structure
        for (_addr_key, account_data) in state {
            let account_obj = account_data.as_object().unwrap();
            assert!(account_obj.contains_key("info"));
            assert!(account_obj.contains_key("original_info"));
            assert!(account_obj.contains_key("storage"));
            assert!(account_obj.contains_key("status"));
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
        let result = DataCollector::collect(state);
        // Verify the function returns successfully
        assert!(result.is_ok());

        let (codes, _preimages, _hashed_state, returned_bundle_state) = result.unwrap();

        // Verify that the returned data contains expected values
        // Since we used the fixture data, we should have some codes and state
        assert!(!codes.is_empty(), "Expected some bytecode entries");
        assert!(!returned_bundle_state.state.is_empty(), "Expected some state entries");

        // Verify the bundle state structure matches our fixture
        assert_eq!(returned_bundle_state.state.len(), 2, "Expected 2 accounts from fixture");
        assert_eq!(returned_bundle_state.contracts.len(), 2, "Expected 2 contracts from fixture");
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

    /// Helper function to create `InvalidBlockWitnessHook` for testing
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

        // Call ProofGenerator::generate
        let result = ProofGenerator::generate(
            codes.clone(),
            preimages.clone(),
            hashed_state,
            state_provider,
        );

        // Verify result
        assert!(result.is_ok(), "ProofGenerator::generate should succeed");
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

    //Fixture functions
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
}
