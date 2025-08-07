use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, Address, B256, U256};
use alloy_rpc_types_debug::ExecutionWitness;
use pretty_assertions::Comparison;
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedHeader};
use reth_provider::{BlockExecutionOutput, ChainSpecProvider, StateProviderFactory};
use reth_revm::{database::StateProviderDatabase, db::BundleState, state::AccountInfo};
use reth_rpc_api::DebugApiClient;
use reth_tracing::tracing::warn;
use reth_trie::{updates::TrieUpdates, HashedStorage};
use revm_bytecode::Bytecode;
use revm_database::states::{
    reverts::{AccountInfoRevert, RevertToSlot},
    AccountStatus, StorageSlot,
};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Debug, fs::File, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
struct AccountRevertSorted {
    pub account: AccountInfoRevert,
    pub storage: BTreeMap<U256, RevertToSlot>,
    pub previous_status: AccountStatus,
    pub wipe_storage: bool,
}

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

impl BundleStateSorted {
    fn from_bundle_state(bundle_state: &BundleState) -> Self {
        let state = bundle_state
            .state
            .clone()
            .into_iter()
            .map(|(address, account)| {
                {
                    (
                        address,
                        BundleAccountSorted {
                            info: account.info,
                            original_info: account.original_info,
                            status: account.status,
                            storage: BTreeMap::from_iter(account.storage),
                        },
                    )
                }
            })
            .collect();

        let contracts = BTreeMap::from_iter(bundle_state.contracts.clone());

        let reverts = bundle_state
            .reverts
            .iter()
            .map(|block| {
                block
                    .iter()
                    .map(|(address, account_revert)| {
                        (
                            *address,
                            AccountRevertSorted {
                                account: account_revert.account.clone(),
                                previous_status: account_revert.previous_status,
                                wipe_storage: account_revert.wipe_storage,
                                storage: BTreeMap::from_iter(account_revert.storage.clone()),
                            },
                        )
                    })
                    .collect()
            })
            .collect();

        let state_size = bundle_state.state_size;
        let reverts_size = bundle_state.reverts_size;

        Self { state, contracts, reverts, state_size, reverts_size }
    }
}

/// Generates a witness for the given block and saves it to a file.
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
    P: StateProviderFactory + ChainSpecProvider + Send + Sync + 'static,
    E: ConfigureEvm<Primitives = N> + 'static,
    N: NodePrimitives,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()>
    where
        N: NodePrimitives,
    {
        // TODO(alexey): unify with `DebugApi::debug_execution_witness`

        let mut executor = self.evm_config.batch_executor(StateProviderDatabase::new(
            self.provider.state_by_block_hash(parent_header.hash())?,
        ));

        executor.execute_one(block)?;

        // Take the bundle state
        let mut db = executor.into_state();
        let mut bundle_state = db.take_bundle();

        // Initialize a map of preimages.
        let mut state_preimages = Vec::default();

        // Get codes
        let codes = db
            .cache
            .contracts
            .values()
            .map(|code| code.original_bytes())
            .chain(
                // cache state does not have all the contracts, especially when
                // a contract is created within the block
                // the contract only exists in bundle state, therefore we need
                // to include them as well
                bundle_state.contracts.values().map(|code| code.original_bytes()),
            )
            .collect();

        // Grab all account proofs for the data accessed during block execution.
        //
        // Note: We grab *all* accounts in the cache here, as the `BundleState` prunes
        // referenced accounts + storage slots.
        let mut hashed_state = db.database.hashed_post_state(&bundle_state);
        for (address, account) in db.cache.accounts {
            let hashed_address = keccak256(address);
            hashed_state
                .accounts
                .insert(hashed_address, account.account.as_ref().map(|a| a.info.clone().into()));

            let storage = hashed_state
                .storages
                .entry(hashed_address)
                .or_insert_with(|| HashedStorage::new(account.status.was_destroyed()));

            if let Some(account) = account.account {
                state_preimages.push(alloy_rlp::encode(address).into());

                for (slot, value) in account.storage {
                    let slot = B256::from(slot);
                    let hashed_slot = keccak256(slot);
                    storage.storage.insert(hashed_slot, value);

                    state_preimages.push(alloy_rlp::encode(slot).into());
                }
            }
        }

        // Generate an execution witness for the aggregated state of accessed accounts.
        // Destruct the cache database to retrieve the state provider.
        let state_provider = db.database.into_inner();
        let state = state_provider.witness(Default::default(), hashed_state.clone())?;

        // Write the witness to the output directory.
        let response =
            ExecutionWitness { state, codes, keys: state_preimages, ..Default::default() };
        let re_executed_witness_path = self.save_file(
            format!("{}_{}.witness.re_executed.json", block.number(), block.hash()),
            &response,
        )?;
        if let Some(healthy_node_client) = &self.healthy_node_client {
            // Compare the witness against the healthy node.
            let healthy_node_witness = futures::executor::block_on(async move {
                DebugApiClient::<()>::debug_execution_witness(
                    healthy_node_client,
                    block.number().into(),
                )
                .await
            })?;

            let healthy_path = self.save_file(
                format!("{}_{}.witness.healthy.json", block.number(), block.hash()),
                &healthy_node_witness,
            )?;

            // If the witnesses are different, write the diff to the output directory.
            if response != healthy_node_witness {
                let filename = format!("{}_{}.witness.diff", block.number(), block.hash());
                let diff_path = self.save_diff(filename, &response, &healthy_node_witness)?;
                warn!(
                    target: "engine::invalid_block_hooks::witness",
                    diff_path = %diff_path.display(),
                    re_executed_path = %re_executed_witness_path.display(),
                    healthy_path = %healthy_path.display(),
                    "Witness mismatch against healthy node"
                );
            }
        }

        // The bundle state after re-execution should match the original one.
        //
        // NOTE: This should not be needed if `Reverts` had a comparison method that sorted first,
        // or otherwise did not care about order.
        //
        // See: https://github.com/bluealloy/revm/issues/1813
        let mut output = output.clone();
        for reverts in output.state.reverts.iter_mut() {
            reverts.sort_by(|left, right| left.0.cmp(&right.0));
        }

        // We also have to sort the `bundle_state` reverts
        for reverts in bundle_state.reverts.iter_mut() {
            reverts.sort_by(|left, right| left.0.cmp(&right.0));
        }

        if bundle_state != output.state {
            let original_path = self.save_file(
                format!("{}_{}.bundle_state.original.json", block.number(), block.hash()),
                &output.state,
            )?;
            let re_executed_path = self.save_file(
                format!("{}_{}.bundle_state.re_executed.json", block.number(), block.hash()),
                &bundle_state,
            )?;

            let filename = format!("{}_{}.bundle_state.diff", block.number(), block.hash());
            // Convert bundle state to sorted struct which has BTreeMap instead of HashMap to
            // have deterministic ordering
            let bundle_state_sorted = BundleStateSorted::from_bundle_state(&bundle_state);
            let output_state_sorted = BundleStateSorted::from_bundle_state(&output.state);

            let diff_path = self.save_diff(filename, &bundle_state_sorted, &output_state_sorted)?;

            warn!(
                target: "engine::invalid_block_hooks::witness",
                diff_path = %diff_path.display(),
                original_path = %original_path.display(),
                re_executed_path = %re_executed_path.display(),
                "Bundle state mismatch after re-execution"
            );
        }

        // Calculate the state root and trie updates after re-execution. They should match
        // the original ones.
        let (re_executed_root, trie_output) =
            state_provider.state_root_with_updates(hashed_state)?;
        if let Some((original_updates, original_root)) = trie_updates {
            if re_executed_root != original_root {
                let filename = format!("{}_{}.state_root.diff", block.number(), block.hash());
                let diff_path = self.save_diff(filename, &re_executed_root, &original_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", ?original_root, ?re_executed_root, diff_path = %diff_path.display(), "State root mismatch after re-execution");
            }

            // If the re-executed state root does not match the _header_ state root, also log that.
            if re_executed_root != block.state_root() {
                let filename =
                    format!("{}_{}.header_state_root.diff", block.number(), block.hash());
                let diff_path = self.save_diff(filename, &re_executed_root, &block.state_root())?;
                warn!(target: "engine::invalid_block_hooks::witness", header_state_root=?block.state_root(), ?re_executed_root, diff_path = %diff_path.display(), "Re-executed state root does not match block state root");
            }

            if &trie_output != original_updates {
                // Trie updates are too big to diff, so we just save the original and re-executed
                let trie_output_sorted = &trie_output.into_sorted_ref();
                let original_updates_sorted = &original_updates.into_sorted_ref();
                let original_path = self.save_file(
                    format!("{}_{}.trie_updates.original.json", block.number(), block.hash()),
                    original_updates_sorted,
                )?;
                let re_executed_path = self.save_file(
                    format!("{}_{}.trie_updates.re_executed.json", block.number(), block.hash()),
                    trie_output_sorted,
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

    /// Saves the diff of two values into a file with the given name in the output directory.
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

    fn save_file<T: Serialize>(&self, filename: String, value: &T) -> eyre::Result<PathBuf> {
        let path = self.output_directory.join(filename);
        File::create(&path)?.write_all(serde_json::to_string(value)?.as_bytes())?;

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
