use std::{collections::HashMap, fmt::Debug, fs::File, io::Write, path::PathBuf};

use alloy_primitives::{keccak256, B256, U256};
use alloy_rpc_types_debug::ExecutionWitness;
use eyre::OptionExt;
use pretty_assertions::Comparison;
use reth_chainspec::ChainSpec;
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{
    system_calls::{apply_beacon_root_contract_call, apply_blockhashes_contract_call},
    ConfigureEvm,
};
use reth_primitives::{Header, Receipt, SealedBlockWithSenders, SealedHeader};
use reth_provider::{BlockExecutionOutput, ChainSpecProvider, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::states::bundle_state::BundleRetention,
    primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg},
    DatabaseCommit, StateBuilder,
};
use reth_rpc_api::DebugApiClient;
use reth_tracing::tracing::warn;
use reth_trie::{updates::TrieUpdates, HashedPostState, HashedStorage};

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
    P: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> + Send + Sync + 'static,
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

        // Apply pre-block system contract calls.
        apply_beacon_root_contract_call(
            &self.evm_config,
            self.provider.chain_spec().as_ref(),
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;
        apply_blockhashes_contract_call(
            &self.evm_config,
            &self.provider.chain_spec(),
            block.timestamp,
            block.number,
            block.parent_hash,
            &mut evm,
        )?;

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

        // Merge all state transitions
        db.merge_transitions(BundleRetention::Reverts);

        // Take the bundle state
        let bundle_state = db.take_bundle();

        // Initialize a map of preimages.
        let mut state_preimages = HashMap::new();

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
        let response = ExecutionWitness { state, keys: Some(state_preimages) };
        File::create_new(self.output_directory.join(format!(
            "{}_{}.json",
            block.number,
            block.hash()
        )))?
        .write_all(serde_json::to_string(&response)?.as_bytes())?;

        // The bundle state after re-execution should match the original one.
        if bundle_state != output.state {
            let filename = format!("{}_{}.bundle_state.diff", block.number, block.hash());
            let path = self.save_diff(filename, &bundle_state, &output.state)?;
            warn!(target: "engine::invalid_block_hooks::witness", path = %path.display(), "Bundle state mismatch after re-execution");
        }

        // Calculate the state root and trie updates after re-execution. They should match
        // the original ones.
        let (state_root, trie_output) = state_provider.state_root_with_updates(hashed_state)?;
        if let Some(trie_updates) = trie_updates {
            if state_root != trie_updates.1 {
                let filename = format!("{}_{}.state_root.diff", block.number, block.hash());
                let path = self.save_diff(filename, &state_root, &trie_updates.1)?;
                warn!(target: "engine::invalid_block_hooks::witness", path = %path.display(), "State root mismatch after re-execution");
            }

            if &trie_output != trie_updates.0 {
                let filename = format!("{}_{}.trie_updates.diff", block.number, block.hash());
                let path = self.save_diff(filename, &trie_output, trie_updates.0)?;
                warn!(target: "engine::invalid_block_hooks::witness", path = %path.display(), "Trie updates mismatch after re-execution");
            }
        }

        if let Some(healthy_node_client) = &self.healthy_node_client {
            // Compare the witness against the healthy node.
            let healthy_node_witness = futures::executor::block_on(async move {
                DebugApiClient::debug_execution_witness(
                    healthy_node_client,
                    block.number.into(),
                    true,
                )
                .await
            })?;

            // Write the healthy node witness to the output directory.
            File::create_new(self.output_directory.join(format!(
                "{}_{}.healthy_witness.json",
                block.number,
                block.hash()
            )))?
            .write_all(serde_json::to_string(&healthy_node_witness)?.as_bytes())?;

            // If the witnesses are different, write the diff to the output directory.
            if response != healthy_node_witness {
                let filename = format!("{}_{}.healthy_witness.diff", block.number, block.hash());
                let path = self.save_diff(filename, &response, &healthy_node_witness)?;
                warn!(target: "engine::invalid_block_hooks::witness", path = %path.display(), "Witness mismatch against healthy node");
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
        File::create_new(&path)?.write_all(diff.to_string().as_bytes())?;

        Ok(path)
    }
}

impl<P, EvmConfig> InvalidBlockHook for InvalidBlockWitnessHook<P, EvmConfig>
where
    P: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> + Send + Sync + 'static,
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
