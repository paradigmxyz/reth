//! Optimism block execution strategy,

use crate::OptimismEvmConfig;
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_evm::{
    execute::{
        BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory, ProviderError,
    },
    system_calls::{OnStateHook, SystemCaller},
    ConfigureEvm, ConfigureEvmEnv,
};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::validate_block_post_execution;
use reth_primitives::{BlockWithSenders, Header, Receipt, Request};
use reth_revm::{
    db::{states::bundle_state::BundleRetention, BundleState},
    Database, State,
};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, U256};
use std::{fmt::Display, sync::Arc};

/// Factory for [`OpExecutionStrategy`].
#[derive(Debug, Clone)]
pub struct OpExecutionStrategyFactory<EvmConfig = OptimismEvmConfig> {
    /// The chainspec
    chain_spec: Arc<OpChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
}

impl OpExecutionStrategyFactory {
    /// Creates a new default optimism executor strategy factory.
    pub fn optimism(chain_spec: Arc<OpChainSpec>) -> Self {
        Self::new(chain_spec.clone(), OptimismEvmConfig::new(chain_spec))
    }
}

impl<EvmConfig> OpExecutionStrategyFactory<EvmConfig> {
    /// Creates a new executor strategy factory.
    pub const fn new(chain_spec: Arc<OpChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config }
    }
}

impl BlockExecutionStrategyFactory for OpExecutionStrategyFactory {
    type Strategy<DB: Database<Error: Into<ProviderError> + Display>> = OpExecutionStrategy<DB>;

    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        OpExecutionStrategy::new(state, self.chain_spec.clone(), self.evm_config.clone())
    }
}

/// Block execution strategy for Optimism.
#[allow(missing_debug_implementations)]
pub struct OpExecutionStrategy<DB, EvmConfig = OptimismEvmConfig> {
    /// The chainspec
    chain_spec: Arc<OpChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
    /// Current state for block execution.
    state: State<DB>,
    /// Utility to call system smart contracts.
    system_caller: SystemCaller<EvmConfig, OpChainSpec>,
}

impl<DB> OpExecutionStrategy<DB> {
    /// Creates a new [`OpExecutionStrategy`]
    pub fn new(
        state: State<DB>,
        chain_spec: Arc<OpChainSpec>,
        evm_config: OptimismEvmConfig,
    ) -> Self {
        let system_caller = SystemCaller::new(evm_config.clone(), (*chain_spec).clone());
        Self { state, chain_spec, evm_config, system_caller }
    }
}

impl<DB> OpExecutionStrategy<DB> {
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// Caution: this does not initialize the tx environment.
    fn evm_env_for_block(&self, header: &Header, total_difficulty: U256) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.evm_config.fill_cfg_and_block_env(&mut cfg, &mut block_env, header, total_difficulty);

        EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default())
    }
}

impl<DB> BlockExecutionStrategy<DB> for OpExecutionStrategy<DB>
where
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), Self::Error> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            (*self.chain_spec).is_spurious_dragon_active_at_block(block.header.number);
        self.state.set_state_clear_flag(state_clear_flag);

        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);

        self.system_caller.apply_pre_execution_changes(block, &mut evm)?;

        Ok(())
    }

    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), Self::Error> {
        Ok((vec![], 0))
    }

    fn apply_post_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        receipts: &[Receipt],
    ) -> Result<Vec<Request>, Self::Error> {
        Ok(vec![])
    }

    fn state_ref(&self) -> &State<DB> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut State<DB> {
        &mut self.state
    }

    fn with_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.system_caller.with_state_hook(hook);
    }

    fn finish(&mut self) -> BundleState {
        self.state.merge_transitions(BundleRetention::Reverts);
        self.state.take_bundle()
    }

    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders,
        receipts: &[Receipt],
        _requests: &[Request],
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec.clone(), receipts)
    }
}
