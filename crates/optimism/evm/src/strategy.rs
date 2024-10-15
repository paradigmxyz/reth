//! Optimism block execution strategy,

use crate::{l1::ensure_create2_deployer, OptimismBlockExecutionError, OptimismEvmConfig};
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_evm::{
    execute::{
        BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory,
        BlockValidationError, ProviderError,
    },
    system_calls::{OnStateHook, SystemCaller},
    ConfigureEvm, ConfigureEvmEnv,
};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::validate_block_post_execution;
use reth_optimism_forks::OptimismHardfork;
use reth_primitives::{BlockWithSenders, Header, Receipt, Request, TxType};
use reth_revm::{
    db::{states::bundle_state::BundleRetention, BundleState},
    state_change::post_block_balance_increments,
    Database, State,
};
use revm_primitives::{
    db::DatabaseCommit, BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState, U256,
};
use std::{fmt::Display, sync::Arc};
use tracing::trace;

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

        self.system_caller.apply_beacon_root_contract_call(
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        ensure_create2_deployer(self.chain_spec.clone(), block.timestamp, evm.db_mut())
            .map_err(|_| OptimismBlockExecutionError::ForceCreate2DeployerFail)?;

        Ok(())
    }

    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), Self::Error> {
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);

        let is_regolith =
            self.chain_spec.fork(OptimismHardfork::Regolith).active_at_timestamp(block.timestamp);

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.transactions.len());
        for (sender, transaction) in block.transactions_with_sender() {
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas &&
                (is_regolith || !transaction.is_system_transaction())
            {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            // An optimism block should never contain blob transactions.
            if matches!(transaction.tx_type(), TxType::Eip4844) {
                return Err(OptimismBlockExecutionError::BlobTransactionRejected.into())
            }

            // Cache the depositor account prior to the state transition for the deposit nonce.
            //
            // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
            // were not introduced in Bedrock. In addition, regular transactions don't have deposit
            // nonces, so we don't need to touch the DB for those.
            let depositor = (is_regolith && transaction.is_deposit())
                .then(|| {
                    evm.db_mut()
                        .load_cache_account(*sender)
                        .map(|acc| acc.account_info().unwrap_or_default())
                })
                .transpose()
                .map_err(|_| OptimismBlockExecutionError::AccountLoadFailed(*sender))?;

            self.evm_config.fill_tx_env(evm.tx_mut(), transaction, *sender);

            // Execute transaction.
            let result_and_state = evm.transact().map_err(move |err| {
                let new_err = err.map_db_err(|e| e.into());
                // Ensure hash is calculated for error log, if not already done
                BlockValidationError::EVM {
                    hash: transaction.recalculate_hash(),
                    error: Box::new(new_err),
                }
            })?;

            trace!(
                target: "evm",
                ?transaction,
                "Executed transaction"
            );
            self.system_caller.on_state(&result_and_state);
            let ResultAndState { result, state } = result_and_state;
            evm.db_mut().commit(state);

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                // Success flag was added in `EIP-658: Embedding transaction status code in
                // receipts`.
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.into_logs(),
                deposit_nonce: depositor.map(|account| account.nonce),
                // The deposit receipt version was introduced in Canyon to indicate an update to how
                // receipt hashes should be computed when set. The state transition process ensures
                // this is only set for post-Canyon deposit transactions.
                deposit_receipt_version: (transaction.is_deposit() &&
                    self.chain_spec
                        .is_fork_active_at_timestamp(OptimismHardfork::Canyon, block.timestamp))
                .then_some(1),
            });
        }

        Ok((receipts, cumulative_gas_used))
    }

    fn apply_post_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        _receipts: &[Receipt],
    ) -> Result<Vec<Request>, Self::Error> {
        let balance_increments =
            post_block_balance_increments(&self.chain_spec.clone(), block, total_difficulty);
        // increment balances
        self.state
            .increment_balances(balance_increments)
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

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
