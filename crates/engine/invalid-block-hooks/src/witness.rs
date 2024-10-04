use std::{collections::HashMap, fmt::Debug, fs::File, io::Write, path::PathBuf};

use alloy_primitives::{keccak256, B256, U256};
use alloy_rpc_types_debug::ExecutionWitness;
use eyre::OptionExt;
use pretty_assertions::Comparison;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{system_calls::SystemCaller, ConfigureEvm};
use reth_primitives::{Header, Receipt, SealedBlockWithSenders, SealedHeader};
use reth_provider::{BlockExecutionOutput, ChainSpecProvider, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::states::bundle_state::BundleRetention,
    primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg},
    state_change::post_block_balance_increments,
    DatabaseCommit, StateBuilder,
};
use reth_rpc_api::DebugApiClient;
use reth_tracing::tracing::warn;
use reth_trie::{updates::TrieUpdates, HashedPostState, HashedStorage};
use serde::Serialize;

/// Generates a witness for the given block and saves it to a file.
#[derive(Debug)]
pub struct InvalidBlockWitnessHook<P, EvmConfig> {
    /// The provider to read the historical state and do the EVM execution.
    provider: P,
    /// The EVM configuration to use for the execution.
    evm_config: EvmConfig,
    /// The directory to write the witness to. Additionally, diff files will be written to this
    /// directory in case of failed sanity checks.
    output_directory: PathBuf,
    /// The healthy node client to compare the witness against.
    healthy_node_client: Option<jsonrpsee::http_client::HttpClient>,
}

impl<P, EvmConfig> InvalidBlockWitnessHook<P, EvmConfig> {
    /// Creates a new witness hook.
    pub const fn new(
        provider: P,
        evm_config: EvmConfig,
        output_directory: PathBuf,
        healthy_node_client: Option<jsonrpsee::http_client::HttpClient>,
    ) -> Self {
        Self { provider, evm_config, output_directory, healthy_node_client }
    }
}

impl<P, EvmConfig> InvalidBlockWitnessHook<P, EvmConfig>
where
    P: StateProviderFactory
        + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
        + Send
        + Sync
        + 'static,
    EvmConfig: ConfigureEvm<Header = Header>,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
        // TODO(alexey): unify with `DebugApi::debug_execution_witness`

        // Setup database.
        let mut db = StateBuilder::new()
            .with_database(StateProviderDatabase::new(
                self.provider.state_by_block_hash(parent_header.hash())?,
            ))
            .with_bundle_update()
            .build();

        // Setup environment for the execution.
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.evm_config.fill_cfg_and_block_env(&mut cfg, &mut block_env, block.header(), U256::MAX);

        // Setup EVM
        let mut evm = self.evm_config.evm_with_env(
            &mut db,
            EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default()),
        );

        let mut system_caller = SystemCaller::new(&self.evm_config, self.provider.chain_spec());

        // Apply pre-block system contract calls.
        system_caller.apply_pre_execution_changes(&block.clone().unseal(), &mut evm)?;

        // Re-execute all of the transactions in the block to load all touched accounts into
        // the cache DB.
        for tx in block.transactions() {
            self.evm_config.fill_tx_env(
                evm.tx_mut(),
                tx,
                tx.recover_signer().ok_or_eyre("failed to recover sender")?,
            );
            let result = evm.transact()?;
            evm.db_mut().commit(result.state);
        }

        drop(evm);

        // use U256::MAX here for difficulty, because fetching it is annoying
        // NOTE: This is not mut because we are not doing the DAO irregular state change here
        let balance_increments = post_block_balance_increments(
            self.provider.chain_spec().as_ref(),
            &block.block.clone().unseal(),
            U256::MAX,
        );

        // increment balances
        db.increment_balances(balance_increments)?;

        // Merge all state transitions
        db.merge_transitions(BundleRetention::Reverts);

        // Take the bundle state
        let mut bundle_state = db.take_bundle();

        // Initialize a map of preimages.
        let mut state_preimages = HashMap::default();

        // Grab all account proofs for the data accessed during block execution.
        //
        // Note: We grab *all* accounts in the cache here, as the `BundleState` prunes
        // referenced accounts + storage slots.
        let mut hashed_state = HashedPostState::from_bundle_state(&bundle_state.state);
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
                state_preimages.insert(hashed_address, alloy_rlp::encode(address).into());

                for (slot, value) in account.storage {
                    let slot = B256::from(slot);
                    let hashed_slot = keccak256(slot);
                    storage.storage.insert(hashed_slot, value);

                    state_preimages.insert(hashed_slot, alloy_rlp::encode(slot).into());
                }
            }
        }

        // Generate an execution witness for the aggregated state of accessed accounts.
        // Destruct the cache database to retrieve the state provider.
        let state_provider = db.database.into_inner();
        let state = state_provider.witness(Default::default(), hashed_state.clone())?;

        // Write the witness to the output directory.
        let response = ExecutionWitness {
            state: HashMap::from_iter(state),
            codes: Default::default(),
            keys: Some(state_preimages),
        };
        let re_executed_witness_path = self.save_file(
            format!("{}_{}.witness.re_executed.json", block.number, block.hash()),
            &response,
        )?;
        if let Some(healthy_node_client) = &self.healthy_node_client {
            // Compare the witness against the healthy node.
            let healthy_node_witness = futures::executor::block_on(async move {
                DebugApiClient::debug_execution_witness(healthy_node_client, block.number.into())
                    .await
            })?;

            let healthy_path = self.save_file(
                format!("{}_{}.witness.healthy.json", block.number, block.hash()),
                &healthy_node_witness,
            )?;

            // If the witnesses are different, write the diff to the output directory.
            if response != healthy_node_witness {
                let filename = format!("{}_{}.witness.diff", block.number, block.hash());
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
                format!("{}_{}.bundle_state.original.json", block.number, block.hash()),
                &output.state,
            )?;
            let re_executed_path = self.save_file(
                format!("{}_{}.bundle_state.re_executed.json", block.number, block.hash()),
                &bundle_state,
            )?;

            let filename = format!("{}_{}.bundle_state.diff", block.number, block.hash());
            let diff_path = self.save_diff(filename, &bundle_state, &output.state)?;

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
                let filename = format!("{}_{}.state_root.diff", block.number, block.hash());
                let diff_path = self.save_diff(filename, &re_executed_root, &original_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", ?original_root, ?re_executed_root, diff_path = %diff_path.display(), "State root mismatch after re-execution");
            }

            // If the re-executed state root does not match the _header_ state root, also log that.
            if re_executed_root != block.state_root {
                let filename = format!("{}_{}.header_state_root.diff", block.number, block.hash());
                let diff_path = self.save_diff(filename, &re_executed_root, &block.state_root)?;
                warn!(target: "engine::invalid_block_hooks::witness", header_state_root=?block.state_root, ?re_executed_root, diff_path = %diff_path.display(), "Re-executed state root does not match block state root");
            }

            if &trie_output != original_updates {
                // Trie updates are too big to diff, so we just save the original and re-executed
                let original_path = self.save_file(
                    format!("{}_{}.trie_updates.original.json", block.number, block.hash()),
                    original_updates,
                )?;
                let re_executed_path = self.save_file(
                    format!("{}_{}.trie_updates.re_executed.json", block.number, block.hash()),
                    &trie_output,
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

impl<P, EvmConfig> InvalidBlockHook for InvalidBlockWitnessHook<P, EvmConfig>
where
    P: StateProviderFactory
        + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
        + Send
        + Sync
        + 'static,
    EvmConfig: ConfigureEvm<Header = Header>,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        output: &BlockExecutionOutput<Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        if let Err(err) = self.on_invalid_block(parent_header, block, output, trie_updates) {
            warn!(target: "engine::invalid_block_hooks::witness", %err, "Failed to invoke hook");
        }
    }
}
