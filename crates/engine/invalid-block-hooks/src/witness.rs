use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, B256, Bytes};
use alloy_rpc_types_debug::ExecutionWitness;
use pretty_assertions::Comparison;
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_provider::{BlockExecutionOutput, ChainSpecProvider, StateProviderFactory, StateProvider};
use reth_revm::{database::StateProviderDatabase, db::{BundleState, State}};
use reth_rpc_api::DebugApiClient;
use reth_tracing::tracing::warn;
use reth_trie::{updates::TrieUpdates, HashedStorage};

use std::{collections::BTreeMap, fmt::Debug, fs::File, io::Write, path::PathBuf};

/// A specialized map for collecting state data with automatic sorting and deduplication.
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
    fn collect(mut db: State<StateProviderDatabase<Box<dyn StateProvider>>>) -> eyre::Result<(BTreeMap<B256, Bytes>, BTreeMap<B256, Bytes>, reth_trie::HashedPostState, BundleState)> {
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
            hashed_state.accounts.insert(hashed_address, account.account.as_ref().map(|a| a.info.clone().into()));

            if let Some(account_data) = account.account {
                preimages.insert(hashed_address, alloy_rlp::encode(address).into());
                let storage = hashed_state.storages.entry(hashed_address).or_insert_with(|| HashedStorage::new(account.status.was_destroyed()));

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
    fn generate(codes: BTreeMap<B256, Bytes>, preimages: BTreeMap<B256, Bytes>, hashed_state: reth_trie::HashedPostState, state_provider: Box<dyn StateProvider>) -> eyre::Result<ExecutionWitness> {
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
        let re_executed_witness_path = self.save_json(&format!("{}.witness.re_executed.json", block_prefix), witness)?;
        
        if let Some(healthy_node_client) = &self.healthy_node_client {
            let healthy_node_witness = futures::executor::block_on(async move {
                DebugApiClient::<()>::debug_execution_witness(healthy_node_client, block_number.into()).await
            })?;

            let healthy_path = self.save_json(&format!("{}.witness.healthy.json", block_prefix), &healthy_node_witness)?;

            if witness != &healthy_node_witness {
                let diff_path = self.save_diff(&format!("{}.witness.diff", block_prefix), witness, &healthy_node_witness)?;
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
            let original_path = self.save_json(&format!("{}.bundle_state.original.json", block_prefix), original_state)?;
            let re_executed_path = self.save_json(&format!("{}.bundle_state.re_executed.json", block_prefix), re_executed_state)?;

            // Convert bundle state to sorted format for deterministic comparison
            let bundle_state_sorted = sort_bundle_state_for_comparison(re_executed_state);
            let output_state_sorted = sort_bundle_state_for_comparison(original_state);
            let diff_path = self.save_diff(&format!("{}.bundle_state.diff", block_prefix), &bundle_state_sorted, &output_state_sorted)?;

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
        let (re_executed_root, trie_output) = state_provider.state_root_with_updates(hashed_state)?;
        
        if let Some((original_updates, original_root)) = trie_updates {
            if re_executed_root != original_root {
                let diff_path = self.save_diff(&format!("{}.state_root.diff", block_prefix), &re_executed_root, &original_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", ?original_root, ?re_executed_root, diff_path = %diff_path.display(), "State root mismatch after re-execution");
            }

            if re_executed_root != block.state_root() {
                let diff_path = self.save_diff(&format!("{}.header_state_root.diff", block_prefix), &re_executed_root, &block.state_root())?;
                warn!(target: "engine::invalid_block_hooks::witness", header_state_root=?block.state_root(), ?re_executed_root, diff_path = %diff_path.display(), "Re-executed state root does not match block state root");
            }

            if &trie_output != original_updates {
                let original_path = self.save_json(&format!("{}.trie_updates.original.json", block_prefix), &original_updates.into_sorted_ref())?;
                let re_executed_path = self.save_json(&format!("{}.trie_updates.re_executed.json", block_prefix), &trie_output.into_sorted_ref())?;
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

        self.validate_state_root_and_trie(parent_header, block, &bundle_state, trie_updates, &block_prefix)?;

        Ok(())
    }

    fn save_json<T: serde::Serialize>(&self, filename: &str, data: &T) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let content = serde_json::to_string_pretty(data)?;
        File::create(&path)?.write_all(content.as_bytes())?;
        Ok(path)
    }

    fn save_diff<T: std::fmt::Debug>(&self, filename: &str, old: &T, new: &T) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        let content = Comparison::new(old, new).to_string();
        File::create(&path)?.write_all(content.as_bytes())?;
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