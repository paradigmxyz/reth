use crate::Runtime;
use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_debug::ExecutionWitness;
use evm2::{bytecode::Bytecode, evm::AccountInfo};
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{database::StateProviderDatabase, ConfigureEvm, Executor};
use reth_execution_types::BundleSource;
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_provider::{BlockExecutionOutput, StateProvider, StateProviderFactory};
use reth_rpc_api::DebugApiClient;
use reth_trie::{
    bundle_state::{AccountInfoRevert, AccountStatus, BundleState, RevertToSlot, StorageSlot},
    updates::TrieUpdates,
};
use serde::Serialize;
use similar::TextDiff;
use std::{
    collections::BTreeMap,
    fmt::Debug,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use tracing::{debug, error, warn};

/// Time budget for computing a diff before the remaining lines are reported as replaced.
///
/// The bundle state of a large block renders to hundreds of thousands of lines. Bound the
/// diff search so that one block cannot monopolize the background worker.
const DIFF_TIMEOUT: Duration = Duration::from_secs(1);

/// Number of invalid blocks whose output files are kept in the output directory.
const MAX_RETAINED_BLOCKS: usize = 100;

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
                        storage: acc.storage.iter().map(|(k, v)| (*k, *v)).collect(),
                        status: acc.status,
                    },
                )
            })
            .collect(),
        contracts: bundle_state.contracts.iter().map(|(k, v)| (*k, v.clone())).collect(),
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
                                storage: rev.storage.iter().map(|(k, v)| (*k, *v)).collect(),
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

/// Hook for generating execution witnesses when invalid blocks are detected.
///
/// This hook captures the execution state and generates witness data that can be used
/// for debugging and analysis of invalid block execution. Diagnostics run on a blocking task;
/// additional invalid blocks are skipped while a task is running or queued. Only the output of
/// the highest invalid blocks is kept.
#[derive(Clone, Debug)]
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
    /// Runtime for background diagnostics and shutdown-aware RPC requests.
    runtime: Runtime,
    /// Allows at most one diagnostic task, including across clones of the hook.
    task_permit: Arc<Semaphore>,
}

impl<P, E> InvalidBlockWitnessHook<P, E> {
    /// Creates a witness hook that runs diagnostics on the given runtime.
    ///
    /// If `healthy_node_client` is `None`, no remote witness comparison is performed.
    pub fn new(
        provider: P,
        evm_config: E,
        output_directory: PathBuf,
        healthy_node_client: Option<jsonrpsee::http_client::HttpClient>,
        runtime: Runtime,
    ) -> Self {
        Self {
            provider,
            evm_config,
            output_directory,
            healthy_node_client,
            runtime,
            task_permit: Arc::new(Semaphore::new(1)),
        }
    }
}

impl<P, E, N> InvalidBlockWitnessHook<P, E>
where
    P: StateProviderFactory + 'static,
    E: ConfigureEvm<Primitives = N> + 'static,
    N: NodePrimitives,
{
    /// Re-executes the block and collects execution data
    fn re_execute_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
    ) -> eyre::Result<(ExecutionWitness, BundleState, reth_trie::HashedPostState)> {
        let state_provider = self.provider.state_by_block_hash(parent_header.hash())?;
        let mut database = evm2::evm::CacheDB::new(evm2::evm::Db::new(StateProviderDatabase::new(
            state_provider.as_ref().into_evm_state_provider(),
        )));
        let output = self.evm_config.executor(&mut database).execute(block)?;
        let hashed_state = state_provider.hashed_post_state(&output.state)?;
        database.commit_source(&BundleSource(&output.state));
        let witness = reth_evm::witness::ExecutionWitnessRecord::new(&database)
            .into_execution_witness_without_headers(
                state_provider.as_ref(),
                reth_trie::ExecutionWitnessMode::Legacy,
            )?;
        let bundle_state = output.state;

        Ok((witness, bundle_state, hashed_state))
    }

    /// Handles witness generation, saving, and comparison with healthy node
    async fn handle_witness_operations(
        &self,
        witness: &ExecutionWitness,
        block_prefix: &str,
        block_number: u64,
    ) -> eyre::Result<()> {
        let filename = format!("{}.witness.re_executed.json", block_prefix);
        let re_executed_witness_path = self.save_file(filename, witness)?;

        if let Some(healthy_node_client) = &self.healthy_node_client {
            let healthy_node_witness = DebugApiClient::<()>::debug_execution_witness(
                healthy_node_client,
                block_number.into(),
                None,
            )
            .await?;

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
            let original_filename = format!("{}.bundle_state.original.json", block_prefix);
            let original_path = self.save_file(original_filename, original_state)?;
            let re_executed_filename = format!("{}.bundle_state.re_executed.json", block_prefix);
            let re_executed_path = self.save_file(re_executed_filename, re_executed_state)?;

            // Convert bundle state to sorted format for deterministic comparison
            let bundle_state_sorted = sort_bundle_state_for_comparison(re_executed_state);
            let output_state_sorted = sort_bundle_state_for_comparison(original_state);
            let filename = format!("{}.bundle_state.diff", block_prefix);
            let diff_path = self.save_diff(filename, &output_state_sorted, &bundle_state_sorted)?;

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
        hashed_state: reth_trie::HashedPostState,
        trie_updates: Option<(&TrieUpdates, B256)>,
        block_prefix: &str,
    ) -> eyre::Result<()> {
        let state_provider = self.provider.state_by_block_hash(parent_header.hash())?;
        let (re_executed_root, trie_output) =
            state_provider.state_root_with_updates(hashed_state)?;

        if let Some((original_updates, original_root)) = trie_updates {
            if re_executed_root != original_root {
                let filename = format!("{}.state_root.diff", block_prefix);
                let diff_path = self.save_diff(filename, &original_root, &re_executed_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", ?original_root, ?re_executed_root, diff_path = %diff_path.display(), "State root mismatch after re-execution");
            }

            if re_executed_root != block.state_root() {
                let filename = format!("{}.header_state_root.diff", block_prefix);
                let diff_path = self.save_diff(filename, &block.state_root(), &re_executed_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", header_state_root=?block.state_root(), ?re_executed_root, diff_path = %diff_path.display(), "Re-executed state root does not match block state root");
            }

            if &trie_output != original_updates {
                let original_path = self.save_file(
                    format!("{}.trie_updates.original.json", block_prefix),
                    &original_updates.to_sorted_ref(),
                )?;
                let re_executed_path = self.save_file(
                    format!("{}.trie_updates.re_executed.json", block_prefix),
                    &trie_output.to_sorted_ref(),
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

    /// Runs diagnostics on a blocking executor, yielding during the healthy-node RPC request.
    async fn process_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        original_state: &BundleState,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
        if let Err(err) = prune_output(&self.output_directory) {
            error!(target: "engine::invalid_block_hooks::witness", %err, "Failed to prune hook output");
        }

        // TODO(alexey): unify with `DebugApi::debug_execution_witness`
        let (witness, bundle_state, hashed_state) = self.re_execute_block(parent_header, block)?;

        let block_prefix = format!("{}_{}", block.number(), block.hash());
        self.handle_witness_operations(&witness, &block_prefix, block.number()).await?;

        self.validate_bundle_state(&bundle_state, original_state, &block_prefix)?;

        self.validate_state_root_and_trie(
            parent_header,
            block,
            hashed_state,
            trie_updates,
            &block_prefix,
        )?;

        Ok(())
    }

    /// Serializes and saves a value to a JSON file in the output directory
    fn save_file<T: Serialize>(&self, filename: String, value: &T) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let mut writer = BufWriter::new(File::create(&path)?);
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;

        Ok(path)
    }

    /// Compares the debug output of two values and saves their unified line diff to a file in the
    /// output directory.
    ///
    /// Myers' algorithm keeps memory linear in the input size, and [`DIFF_TIMEOUT`] bounds its
    /// running time.
    fn save_diff<T: Debug>(
        &self,
        filename: String,
        original: &T,
        new: &T,
    ) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let original = format!("{original:#?}\n");
        let new = format!("{new:#?}\n");
        let diff = TextDiff::configure().timeout(DIFF_TIMEOUT).diff_lines(&original, &new);
        File::create(&path)?
            .write_all(diff.unified_diff().header("original", "new").to_string().as_bytes())?;

        Ok(path)
    }
}

impl<P, E, N: NodePrimitives> InvalidBlockHook<N> for InvalidBlockWitnessHook<P, E>
where
    P: StateProviderFactory + Clone + 'static,
    E: ConfigureEvm<Primitives = N> + 'static,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        // Reserve capacity before cloning potentially large diagnostic inputs.
        let Ok(permit) = self.task_permit.clone().try_acquire_owned() else {
            metrics::counter!("engine.invalid_block_hooks.witness.skipped_blocks_total")
                .increment(1);
            debug!(target: "engine::invalid_block_hooks::witness", block_hash = %block.hash(), "Skipping diagnostics while witness hook is busy");
            return
        };

        let hook = self.clone();
        let parent_header = parent_header.clone();
        let block = block.clone();
        let original_state = output.state.clone();
        let trie_updates = trie_updates.map(|(updates, root)| (updates.clone(), root));
        self.runtime.spawn_blocking_task(async move {
            let _permit = permit;
            debug!(target: "engine::invalid_block_hooks::witness", block_hash = %block.hash(), "Starting witness diagnostics");
            if let Err(err) = hook
                .process_invalid_block(
                    &parent_header,
                    &block,
                    &original_state,
                    trie_updates.as_ref().map(|(updates, root)| (updates, *root)),
                )
                .await
            {
                error!(target: "engine::invalid_block_hooks::witness", block_hash = %block.hash(), %err, "Failed to invoke hook");
            }
            debug!(target: "engine::invalid_block_hooks::witness", block_hash = %block.hash(), "Finished witness diagnostics");
        });
    }
}

/// Removes the output files of all but the highest blocks, so that the output of
/// [`MAX_RETAINED_BLOCKS`] blocks remains once the next block is written.
///
/// Only files named `{block_number}_{block_hash}.*` are considered, others are left alone.
fn prune_output(output_directory: &Path) -> eyre::Result<()> {
    let mut blocks = BTreeMap::<(u64, String), Vec<PathBuf>>::new();
    for entry in reth_fs_util::read_dir(output_directory)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if let Some((prefix, _)) = file_name.to_str().and_then(|name| name.split_once('.')) &&
            let Some((number, hash)) = prefix.split_once('_') &&
            let Ok(number) = number.parse::<u64>() &&
            hash.parse::<B256>().is_ok()
        {
            blocks.entry((number, prefix.to_string())).or_default().push(entry.path());
        }
    }

    for path in blocks.into_values().rev().skip(MAX_RETAINED_BLOCKS - 1).flatten() {
        reth_fs_util::remove_file(path)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{map::HashMap, Bytes, B256, U256};
    use evm2::evm::{CacheDB, EmptyDB};
    use jsonrpsee::{
        core::RpcResult,
        http_client::{HttpClient, HttpClientBuilder},
        server::{ServerBuilder, ServerHandle},
        RpcModule,
    };
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::{Block, EthPrimitives};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use reth_trie::bundle_state::{AccountRevert, BundleAccount};
    use tempfile::TempDir;
    use tokio::{sync::Notify, time::timeout};

    use reth_testing_utils::generators::{self, random_block, random_eoa_accounts, BlockParams};
    use std::collections::HashSet;

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
                    ..Default::default()
                }),
                original_info: (i == 0).then(|| AccountInfo {
                    balance: account.balance.checked_div(U256::from(2)).unwrap_or(U256::ZERO),
                    code_hash: account.bytecode_hash.unwrap_or_default(),
                    code: None,
                    ..Default::default()
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

    fn hashed_state_for_bundle_state(bundle_state: BundleState) -> reth_trie::HashedPostState {
        reth_trie::HashedPostState::from_bundle_state::<reth_trie::KeccakKeyHasher>(
            &bundle_state.state,
        )
    }

    #[test]
    fn test_sort_bundle_state_for_comparison() {
        let sorted = sort_bundle_state_for_comparison(&create_bundle_state());
        assert_eq!(sorted.state_size, 3);
        assert_eq!(sorted.reverts_size, 2);
        assert_eq!(sorted.state.len(), 3);
        assert_eq!(sorted.contracts.len(), 3);
        assert_eq!(sorted.reverts.len(), 2);
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

        let hook = InvalidBlockWitnessHook::new(
            provider,
            evm_config,
            output_directory.clone(),
            None,
            Runtime::test(),
        );

        (hook, output_directory, temp_dir)
    }

    #[tokio::test]
    async fn test_handle_witness_operations_file_creation() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let witness = ExecutionWitness {
            state: vec![Bytes::from("state")],
            codes: vec![Bytes::from("code")],
            keys: vec![Bytes::from("key")],
            ..Default::default()
        };
        hook.handle_witness_operations(&witness, "block", 0).await.unwrap();
        let saved = std::fs::read(output_dir.join("block.witness.re_executed.json")).unwrap();
        assert_eq!(serde_json::from_slice::<ExecutionWitness>(&saved).unwrap(), witness);
    }

    #[test]
    fn witness_includes_unchanged_account_storage_and_code_reads() {
        let provider = MockEthProvider::default();
        let mut database = CacheDB::<EmptyDB>::default();
        let address = Address::repeat_byte(1);
        let slot = U256::from(7);
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        database.insert_account_info(&address, AccountInfo::default().with_code(code.clone()));
        database.insert_account_storage(&address, &slot, &U256::from(8));
        let witness = reth_evm::witness::ExecutionWitnessRecord::new(&database)
            .into_execution_witness_without_headers(
                &provider,
                reth_trie::ExecutionWitnessMode::Legacy,
            )
            .unwrap();
        assert!(witness.codes.contains(&code.original_bytes()));
        assert!(witness.keys.contains(&Bytes::copy_from_slice(address.as_slice())));
        assert!(witness.keys.contains(&Bytes::copy_from_slice(B256::from(slot).as_slice())));
    }

    #[test]
    fn test_validate_bundle_state_matching() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        for state in [BundleState::default(), create_bundle_state()] {
            hook.validate_bundle_state(&state, &state, "matching").unwrap();
            assert_eq!(output_dir.read_dir().unwrap().count(), 0);
        }
    }

    #[test]
    fn test_validate_bundle_state_mismatch() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let original_state = create_bundle_state();
        let mut modified_state = original_state.clone();
        modified_state.state.values_mut().next().unwrap().info.as_mut().unwrap().nonce ^= 1;

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

        let mut account_nodes = HashMap::default();
        let nibbles = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3]);
        let branch_node = BranchNodeCompact::new(
            0b1010,                       // state_mask
            0b1010,                       // tree_mask - must be subset of state_mask
            0b1000,                       // hash_mask
            vec![B256::repeat_byte(1u8)], // hashes
            None,                         // root_hash
        );
        account_nodes.insert(nibbles, branch_node);

        let mut removed_nodes = HashSet::default();
        removed_nodes.insert(Nibbles::from_nibbles_unchecked([0x4, 0x5, 0x6]));

        TrieUpdates { account_nodes, removed_nodes, storage_tries: HashMap::default() }
    }

    #[test]
    fn test_validate_state_root_and_trie_with_trie_updates() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let bundle_state = create_bundle_state();

        // Generate test data
        let (parent_header, recovered_block) = invalid_block_fixture(2);

        let trie_updates = create_test_trie_updates();
        let original_root = B256::repeat_byte(2u8); // Different from what will be computed
        let block_prefix = "test_state_root_with_trie";
        let hashed_state = hashed_state_for_bundle_state(bundle_state);

        // Test with trie updates - this will likely produce warnings due to mock data
        let result = hook.validate_state_root_and_trie(
            &parent_header,
            &recovered_block,
            hashed_state,
            Some((&trie_updates, original_root)),
            block_prefix,
        );
        result.unwrap();
        assert!(output_dir.join(format!("{block_prefix}.state_root.diff")).exists());
        assert!(output_dir.join(format!("{block_prefix}.trie_updates.original.json")).exists());
        assert!(output_dir.join(format!("{block_prefix}.trie_updates.re_executed.json")).exists());
    }

    #[test]
    fn test_validate_bundle_state_with_different_contract_counts() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let state1 = create_bundle_state();
        let mut state2 = state1.clone();

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
    fn test_save_diff() {
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        for (new, expected) in [(1, ""), (2, "--- original\n+++ new\n@@ -1 +1 @@\n-1\n+2\n")] {
            let path = hook.save_diff("diff".to_string(), &1, &new).unwrap();
            assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
        }
    }

    #[test]
    fn test_validate_state_root_and_trie_without_trie_updates() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let bundle_state = create_bundle_state();

        let (parent_header, recovered_block) = invalid_block_fixture(2);

        let block_prefix = "no_trie_updates_test";
        let hashed_state = hashed_state_for_bundle_state(bundle_state);

        // Test without trie updates (None case)
        let result = hook.validate_state_root_and_trie(
            &parent_header,
            &recovered_block,
            hashed_state,
            None,
            block_prefix,
        );
        result.unwrap();
        assert_eq!(output_dir.read_dir().unwrap().count(), 0);
    }

    #[test]
    fn test_save_diff_large_bundle_state() {
        const ACCOUNTS: usize = 3_000;
        let (hook, _output_dir, _temp_dir) = create_test_hook();
        let original = sort_bundle_state_for_comparison(&create_transfer_bundle_state(ACCOUNTS, 0));
        let new =
            sort_bundle_state_for_comparison(&create_transfer_bundle_state(ACCOUNTS, ACCOUNTS));

        // Every account differs, so the changed region spans the whole state. A quadratic line
        // diff of it would allocate tens of gigabytes.
        let path = hook.save_diff("large_diff_test".to_string(), &original, &new).unwrap();

        let diff = std::fs::read_to_string(path).unwrap();
        let added: HashSet<_> =
            diff.lines().filter_map(|line| line.strip_prefix('+')).map(str::trim).collect();
        for balance in ACCOUNTS..2 * ACCOUNTS {
            assert!(added.contains(format!("balance: {balance},").as_str()));
        }
    }

    /// Creates a `BundleState` of `accounts` funded accounts, offsetting every balance by
    /// `balance_offset`.
    fn create_transfer_bundle_state(accounts: usize, balance_offset: usize) -> BundleState {
        let mut bundle_state = BundleState::default();
        for i in 0..accounts {
            bundle_state.state.insert(
                Address::from_word(B256::from(U256::from(i))),
                BundleAccount {
                    info: Some(AccountInfo {
                        balance: U256::from(i + balance_offset),
                        ..Default::default()
                    }),
                    original_info: None,
                    storage: HashMap::default(),
                    status: AccountStatus::Changed,
                },
            );
        }
        bundle_state.state_size = accounts;
        bundle_state
    }

    #[tokio::test]
    async fn test_on_invalid_block_runs_in_background_and_skips_while_busy() {
        let (mut hook, output_dir, _temp_dir) = create_test_hook();
        let (client, server, requested, respond) = healthy_node_server().await;
        hook.healthy_node_client = Some(client);
        let (parent, block) = invalid_block_fixture(2);
        let (skipped_parent, skipped_block) = invalid_block_fixture(3);
        let output = BlockExecutionOutput::default();
        let healthy_path = output_dir.join(format!("2_{}.witness.healthy.json", block.hash()));
        let skipped_path =
            output_dir.join(format!("3_{}.witness.re_executed.json", skipped_block.hash()));

        hook.on_invalid_block(&parent, &block, &output, None);
        timeout(Duration::from_secs(10), requested.notified()).await.unwrap();
        assert!(!healthy_path.exists());

        // Clones share the admission limit and must not queue another diagnostic job.
        hook.clone().on_invalid_block(&skipped_parent, &skipped_block, &output, None);
        respond.notify_one();
        drop(timeout(Duration::from_secs(10), hook.task_permit.acquire()).await.unwrap().unwrap());
        assert!(healthy_path.exists());
        assert!(!skipped_path.exists());

        hook.healthy_node_client = None;
        hook.on_invalid_block(&skipped_parent, &skipped_block, &output, None);
        drop(timeout(Duration::from_secs(10), hook.task_permit.acquire()).await.unwrap().unwrap());
        assert!(skipped_path.exists());
        server.stop().unwrap();
        timeout(Duration::from_secs(10), server.stopped()).await.unwrap();
    }

    #[tokio::test]
    async fn test_on_invalid_block_releases_capacity_after_error() {
        let (mut hook, output_dir, _temp_dir) = create_test_hook();
        let (parent, block) = invalid_block_fixture(2);
        let output = BlockExecutionOutput::default();
        hook.output_directory = output_dir.join("missing");

        hook.on_invalid_block(&parent, &block, &output, None);
        drop(timeout(Duration::from_secs(10), hook.task_permit.acquire()).await.unwrap().unwrap());
        assert!(!hook.output_directory.exists());

        hook.output_directory = output_dir.clone();
        hook.on_invalid_block(&parent, &block, &output, None);
        drop(timeout(Duration::from_secs(10), hook.task_permit.acquire()).await.unwrap().unwrap());
        assert!(output_dir
            .join(format!("{}_{}.witness.re_executed.json", block.number(), block.hash()))
            .exists());
    }

    #[tokio::test]
    async fn test_on_invalid_block_cancels_rpc_on_shutdown() {
        let (mut hook, output_dir, _temp_dir) = create_test_hook();
        let (client, server, requested, respond) = healthy_node_server().await;
        hook.healthy_node_client = Some(client);
        let (parent, block) = invalid_block_fixture(2);

        hook.on_invalid_block(&parent, &block, &BlockExecutionOutput::default(), None);
        timeout(Duration::from_secs(10), requested.notified()).await.unwrap();
        drop(hook.runtime.initiate_graceful_shutdown().unwrap());
        drop(timeout(Duration::from_secs(10), hook.task_permit.acquire()).await.unwrap().unwrap());
        assert!(!output_dir
            .join(format!("{}_{}.witness.healthy.json", block.number(), block.hash()))
            .exists());

        respond.notify_one();
        server.stop().unwrap();
        timeout(Duration::from_secs(10), server.stopped()).await.unwrap();
    }

    fn invalid_block_fixture(number: u64) -> (SealedHeader<Header>, RecoveredBlock<Block>) {
        let mut rng = generators::rng();
        let parent = generators::random_header(&mut rng, number - 1, None);
        let block = random_block(
            &mut rng,
            number,
            BlockParams { parent: Some(parent.hash()), tx_count: Some(0), ..Default::default() },
        )
        .try_recover()
        .unwrap();
        (parent, block)
    }

    async fn healthy_node_server() -> (HttpClient, ServerHandle, Arc<Notify>, Arc<Notify>) {
        let requested = Arc::new(Notify::new());
        let respond = Arc::new(Notify::new());
        let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let client = HttpClientBuilder::default()
            .build(format!("http://{}", server.local_addr().unwrap()))
            .unwrap();
        let mut module = RpcModule::new((requested.clone(), respond.clone()));
        module
            .register_async_method("debug_executionWitness", |_, context, _| async move {
                context.0.notify_one();
                context.1.notified().await;
                RpcResult::Ok(ExecutionWitness {
                    codes: vec![Bytes::from_static(b"healthy node code")],
                    ..Default::default()
                })
            })
            .unwrap();
        (client, server.start(module), requested, respond)
    }

    #[test]
    fn test_prune_output_keeps_highest_blocks() {
        let temp_dir = TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let blocks = (0..MAX_RETAINED_BLOCKS as u64 + 2)
            .map(|number| {
                let prefix = format!("{number}_{}", B256::with_last_byte(number as u8));
                let files = [
                    output_dir.join(format!("{prefix}.witness.re_executed.json")),
                    output_dir.join(format!("{prefix}.bundle_state.diff")),
                ];
                for path in &files {
                    File::create(path).unwrap();
                }
                files
            })
            .collect::<Vec<_>>();
        let unrelated = output_dir.join("notes.txt");
        File::create(&unrelated).unwrap();

        prune_output(output_dir).unwrap();

        // Room is left for one more block, so the three lowest blocks are removed.
        for (number, files) in blocks.iter().enumerate() {
            for path in files {
                assert_eq!(path.exists(), number >= 3, "{}", path.display());
            }
        }
        assert!(unrelated.exists());
    }

    #[tokio::test]
    async fn test_on_invalid_block_prunes_old_output() {
        let (hook, output_dir, _temp_dir) = create_test_hook();
        let old_blocks = (0..MAX_RETAINED_BLOCKS as u64)
            .map(|number| {
                let path = output_dir.join(format!(
                    "{number}_{}.witness.re_executed.json",
                    B256::with_last_byte(number as u8)
                ));
                File::create(&path).unwrap();
                path
            })
            .collect::<Vec<_>>();

        let (parent_header, block) = invalid_block_fixture(100);
        let output = BlockExecutionOutput::default();
        let permit = hook.task_permit.try_acquire().unwrap();

        // Busy hooks must skip pruning as well as writing.
        hook.on_invalid_block(&parent_header, &block, &output, None);
        assert!(old_blocks[0].exists());
        drop(permit);

        hook.on_invalid_block(&parent_header, &block, &output, None);
        drop(timeout(Duration::from_secs(10), hook.task_permit.acquire()).await.unwrap().unwrap());
        assert!(!old_blocks[0].exists());
        assert!(old_blocks[1..].iter().all(|path| path.exists()));
        assert!(output_dir
            .join(format!("{}_{}.witness.re_executed.json", block.number(), block.hash()))
            .exists());
    }
}
