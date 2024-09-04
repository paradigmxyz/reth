use std::{collections::HashMap, path::PathBuf};

use alloy_rpc_types_debug::ExecutionWitness;
use reth_chainspec::ChainSpec;
use reth_engine_primitives::InvalidBlockHook;
use reth_evm::{
    system_calls::{pre_block_beacon_root_contract_call, pre_block_blockhashes_contract_call},
    ConfigureEvm,
};
use reth_primitives::{keccak256, Receipt, SealedBlockWithSenders, SealedHeader, B256, U256};
use reth_provider::{BlockExecutionOutput, ChainSpecProvider, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::states::bundle_state::BundleRetention,
    primitives::{BlockEnv, CfgEnvWithHandlerCfg, Env, EnvWithHandlerCfg},
    DatabaseCommit, StateBuilder,
};
use reth_trie::{updates::TrieUpdates, HashedPostState, HashedStorage};

/// Generates a witness for the given block and saves it to a file.
#[derive(Debug)]
pub struct Witness<P, EvmConfig> {
    output_directory: PathBuf,
    provider: P,
    evm_config: EvmConfig,
}

impl<P, EvmConfig> Witness<P, EvmConfig> {
    /// Creates a new witness hook.
    pub const fn new(output_directory: PathBuf, provider: P, evm_config: EvmConfig) -> Self {
        Self { output_directory, provider, evm_config }
    }
}

impl<P, EvmConfig> InvalidBlockHook for Witness<P, EvmConfig>
where
    P: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> + Send + Sync + 'static,
    EvmConfig: ConfigureEvm,
{
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader,
        block: &SealedBlockWithSenders,
        _output: &BlockExecutionOutput<Receipt>,
        _trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
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
        let evm_config = self.evm_config.clone();
        evm_config.fill_cfg_and_block_env(
            &mut cfg,
            &mut block_env,
            &self.provider.chain_spec(),
            block.header(),
            U256::MAX,
        );

        // Apply pre-block system contract calls.
        pre_block_beacon_root_contract_call(
            &mut db,
            &evm_config,
            &self.provider.chain_spec(),
            &cfg,
            &block_env,
            block.parent_beacon_block_root,
        )?;
        pre_block_blockhashes_contract_call(
            &mut db,
            &evm_config,
            &self.provider.chain_spec(),
            &cfg,
            &block_env,
            block.parent_hash,
        )?;

        // Re-execute all of the transactions in the block to load all touched accounts into
        // the cache DB.
        for tx in block.clone().into_transactions_ecrecovered() {
            let env = EnvWithHandlerCfg {
                env: Env::boxed(cfg.cfg_env.clone(), block_env.clone(), evm_config.tx_env(&tx)),
                handler_cfg: cfg.handler_cfg,
            };

            let result = self.evm_config.evm_with_env(&mut db, env).transact()?;
            db.commit(result.state);
        }

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
        let witness = state_provider.witness(HashedPostState::default(), hashed_state)?;

        // Write the witness to the output directory.
        let path = self.output_directory.join(format!("{}_{}.json", block.number, block.hash()));
        let response = ExecutionWitness { witness, state_preimages: Some(state_preimages) };
        std::fs::write(path, serde_json::to_string(&response)?)?;

        Ok(())
    }
}
