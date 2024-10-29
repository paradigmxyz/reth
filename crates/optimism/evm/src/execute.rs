//! Optimism block executor.

use crate::{
    l1::ensure_create2_deployer, OpChainSpec, OptimismBlockExecutionError, OptimismEvmConfig,
};
use alloy_primitives::{Address, BlockNumber, B256, U256};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm::{
    execute::{
        BatchExecutor, BlockExecutionError, BlockExecutionInput, BlockExecutionOutput,
        BlockExecutorProvider, BlockValidationError, Executor, ProviderError,
    },
    system_calls::{NoopHook, OnStateHook, SystemCaller},
    ConfigureEvm,
};
use reth_execution_types::ExecutionOutcome;
use reth_optimism_consensus::validate_block_post_execution;
use reth_optimism_forks::OptimismHardfork;
use reth_primitives::{Account, BlockWithSenders, Header, Receipt, Receipts, StorageEntry, TxType};
use reth_prune_types::PruneModes;
use reth_revm::{
    batch::BlockBatchRecord, db::states::bundle_state::BundleRetention,
    state_change::post_block_balance_increments, Evm, State,
};
use revm_primitives::{
    db::{Database, DatabaseCommit},
    BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState,
};
use std::{collections::HashMap, fmt::Display, sync::Arc};
use tracing::trace;

/// Provides executors to execute regular optimism blocks
#[derive(Debug, Clone)]
pub struct OpExecutorProvider<EvmConfig = OptimismEvmConfig> {
    chain_spec: Arc<OpChainSpec>,
    evm_config: EvmConfig,
}

impl OpExecutorProvider {
    /// Creates a new default optimism executor provider.
    pub fn optimism(chain_spec: Arc<OpChainSpec>) -> Self {
        Self::new(chain_spec.clone(), OptimismEvmConfig::new(chain_spec))
    }
}

impl<EvmConfig> OpExecutorProvider<EvmConfig> {
    /// Creates a new executor provider.
    pub const fn new(chain_spec: Arc<OpChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config }
    }
}

impl<EvmConfig> OpExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    fn op_executor<DB>(&self, db: DB) -> OpBlockExecutor<EvmConfig, DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        OpBlockExecutor::new(
            self.chain_spec.clone(),
            self.evm_config.clone(),
            State::builder().with_database(db).with_bundle_update().without_state_clear().build(),
        )
    }
}

impl<EvmConfig> BlockExecutorProvider for OpExecutorProvider<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> =
        OpBlockExecutor<EvmConfig, DB>;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> =
        OpBatchExecutor<EvmConfig, DB>;
    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.op_executor(db)
    }

    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let executor = self.op_executor(db);
        OpBatchExecutor { executor, batch_record: BlockBatchRecord::default() }
    }
}

/// Helper container type for EVM with chain spec.
#[derive(Debug, Clone)]
pub struct OpEvmExecutor<EvmConfig> {
    /// The chainspec
    chain_spec: Arc<OpChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
}

impl<EvmConfig> OpEvmExecutor<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    /// Executes the transactions in the block and returns the receipts.
    ///
    /// This applies the pre-execution changes, and executes the transactions.
    ///
    /// The optional `state_hook` will be executed with the state changes if present.
    ///
    /// # Note
    ///
    /// It does __not__ apply post-execution changes.
    fn execute_pre_and_transactions<Ext, DB, F>(
        &self,
        block: &BlockWithSenders,
        mut evm: Evm<'_, Ext, &mut State<DB>>,
        state_hook: Option<F>,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
        F: OnStateHook,
    {
        let mut system_caller =
            SystemCaller::new(&self.evm_config, &self.chain_spec).with_state_hook(state_hook);

        // apply pre execution changes
        system_caller.apply_beacon_root_contract_call(
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut evm,
        )?;

        // execute transactions
        let is_regolith =
            self.chain_spec.fork(OptimismHardfork::Regolith).active_at_timestamp(block.timestamp);

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        ensure_create2_deployer(self.chain_spec.clone(), block.timestamp, evm.db_mut())
            .map_err(|_| OptimismBlockExecutionError::ForceCreate2DeployerFail)?;

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
                return Err(OptimismBlockExecutionError::BlobTransactionRejected.into());
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
            system_caller.on_state(&result_and_state);
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
        drop(evm);

        Ok((receipts, cumulative_gas_used))
    }
}

/// Types used inside `RevertsInit` to initialize revms reverts.
pub type AccountRevertInit = (Option<Option<Account>>, Vec<StorageEntry>);

/// Type used to initialize revms reverts.
pub type RevertsInit = HashMap<BlockNumber, HashMap<Address, AccountRevertInit>>;

/// A basic Optimism block executor.
///
/// Expected usage:
/// - Create a new instance of the executor.
/// - Execute the block.
#[derive(Debug)]
pub struct OpBlockExecutor<EvmConfig, DB> {
    /// Chain specific evm config that's used to execute a block.
    executor: OpEvmExecutor<EvmConfig>,
    /// The state to use for execution
    state: State<DB>,
}

impl<EvmConfig, DB> OpBlockExecutor<EvmConfig, DB> {
    /// Creates a new Optimism block executor.
    pub const fn new(
        chain_spec: Arc<OpChainSpec>,
        evm_config: EvmConfig,
        state: State<DB>,
    ) -> Self {
        Self { executor: OpEvmExecutor { chain_spec, evm_config }, state }
    }

    /// Returns the chain spec.
    #[inline]
    pub fn chain_spec(&self) -> &ChainSpec {
        &self.executor.chain_spec
    }

    /// Returns mutable reference to the state that wraps the underlying database.
    pub fn state_mut(&mut self) -> &mut State<DB> {
        &mut self.state
    }
}

impl<EvmConfig, DB> OpBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// Caution: this does not initialize the tx environment.
    fn evm_env_for_block(&self, header: &Header, total_difficulty: U256) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.executor.evm_config.fill_cfg_and_block_env(
            &mut cfg,
            &mut block_env,
            header,
            total_difficulty,
        );

        EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default())
    }

    /// Convenience method to invoke `execute_without_verification_with_state_hook` setting the
    /// state hook as `None`.
    fn execute_without_verification(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        self.execute_without_verification_with_state_hook(block, total_difficulty, None::<NoopHook>)
    }

    /// Execute a single block and apply the state changes to the internal state.
    ///
    /// Returns the receipts of the transactions in the block and the total gas used.
    ///
    /// Returns an error if execution fails.
    fn execute_without_verification_with_state_hook<F>(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        state_hook: Option<F>,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError>
    where
        F: OnStateHook,
    {
        // 1. prepare state on new block
        self.on_new_block(&block.header);

        // 2. configure the evm and execute
        let env = self.evm_env_for_block(&block.header, total_difficulty);

        let (receipts, gas_used) = {
            let evm = self.executor.evm_config.evm_with_env(&mut self.state, env);
            self.executor.execute_pre_and_transactions(block, evm, state_hook)
        }?;

        // 3. apply post execution changes
        self.post_execution(block, total_difficulty)?;

        Ok((receipts, gas_used))
    }

    /// Apply settings before a new block is executed.
    pub(crate) fn on_new_block(&mut self, header: &Header) {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag = self.chain_spec().is_spurious_dragon_active_at_block(header.number);
        self.state.set_state_clear_flag(state_clear_flag);
    }

    /// Apply post execution state changes, including block rewards, withdrawals, and irregular DAO
    /// hardfork state change.
    pub fn post_execution(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        let balance_increments =
            post_block_balance_increments(self.chain_spec(), block, total_difficulty);
        // increment balances
        self.state
            .increment_balances(balance_increments)
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

        Ok(())
    }
}

impl<EvmConfig, DB> Executor<DB> for OpBlockExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    /// Executes the block and commits the state changes.
    ///
    /// Returns the receipts of the transactions in the block.
    ///
    /// Returns an error if the block could not be executed or failed verification.
    ///
    /// State changes are committed to the database.
    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_without_verification(block, total_difficulty)?;

        // NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);

        Ok(BlockExecutionOutput {
            state: self.state.take_bundle(),
            receipts,
            requests: vec![],
            gas_used,
        })
    }

    fn execute_with_state_witness<F>(
        mut self,
        input: Self::Input<'_>,
        mut witness: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        let BlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_without_verification(block, total_difficulty)?;

        // NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);
        witness(&self.state);

        Ok(BlockExecutionOutput {
            state: self.state.take_bundle(),
            receipts,
            requests: vec![],
            gas_used,
        })
    }

    fn execute_with_state_hook<F>(
        mut self,
        input: Self::Input<'_>,
        state_hook: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook,
    {
        let BlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_without_verification_with_state_hook(
            block,
            total_difficulty,
            Some(state_hook),
        )?;

        // NOTE: we need to merge keep the reverts for the bundle retention
        self.state.merge_transitions(BundleRetention::Reverts);

        Ok(BlockExecutionOutput {
            state: self.state.take_bundle(),
            receipts,
            requests: vec![],
            gas_used,
        })
    }
}

/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
pub struct OpBatchExecutor<EvmConfig, DB> {
    /// The executor used to execute blocks.
    executor: OpBlockExecutor<EvmConfig, DB>,
    /// Keeps track of the batch and record receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
}

impl<EvmConfig, DB> OpBatchExecutor<EvmConfig, DB> {
    /// Returns the receipts of the executed blocks.
    pub const fn receipts(&self) -> &Receipts {
        self.batch_record.receipts()
    }

    /// Returns mutable reference to the state that wraps the underlying database.
    pub fn state_mut(&mut self) -> &mut State<DB> {
        self.executor.state_mut()
    }
}

/// Type used to initialize revms bundle state.
pub type BundleStateInit =
    HashMap<Address, (Option<Account>, Option<Account>, HashMap<B256, (U256, U256)>)>;

impl<EvmConfig, DB> BatchExecutor<DB> for OpBatchExecutor<EvmConfig, DB>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;

        if self.batch_record.first_block().is_none() {
            self.batch_record.set_first_block(block.number);
        }

        let (receipts, _gas_used) =
            self.executor.execute_without_verification(block, total_difficulty)?;

        validate_block_post_execution(block, self.executor.chain_spec(), &receipts)?;

        // prepare the state according to the prune mode
        let retention = self.batch_record.bundle_retention(block.number);
        self.executor.state.merge_transitions(retention);

        // store receipts in the set
        self.batch_record.save_receipts(receipts)?;

        Ok(())
    }

    fn finalize(mut self) -> Self::Output {
        ExecutionOutcome::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
            self.batch_record.take_requests(),
        )
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        self.batch_record.set_tip(tip);
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.batch_record.set_prune_modes(prune_modes);
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.executor.state.bundle_state.size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpChainSpec;
    use alloy_consensus::TxEip1559;
    use alloy_eips::{eip6110::DepositRequest, eip7002::WithdrawalRequest};
    use alloy_primitives::{
        b256, Address, FixedBytes, Log, LogData, StorageKey, StorageValue, B256,
    };
    use reth_chainspec::{ChainSpecBuilder, MIN_TRANSACTION_GAS};
    use reth_optimism_chainspec::{optimism_deposit_tx_signature, BASE_MAINNET};
    use reth_primitives::{
        Account, Block, BlockBody, Receipts, Request, Requests, Signature, Transaction,
        TransactionSigned, TxType,
    };
    use reth_revm::{
        database::StateProviderDatabase, test_utils::StateProviderTest, L1_BLOCK_CONTRACT,
    };
    use revm::{db::states::BundleState, primitives::AccountInfo};
    use std::{collections::HashMap, str::FromStr};

    fn create_op_state_provider() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let l1_block_contract_account =
            Account { balance: U256::ZERO, bytecode_hash: None, nonce: 1 };

        let mut l1_block_storage = HashMap::default();
        // base fee
        l1_block_storage.insert(StorageKey::with_last_byte(1), StorageValue::from(1000000000));
        // l1 fee overhead
        l1_block_storage.insert(StorageKey::with_last_byte(5), StorageValue::from(188));
        // l1 fee scalar
        l1_block_storage.insert(StorageKey::with_last_byte(6), StorageValue::from(684000));
        // l1 free scalars post ecotone
        l1_block_storage.insert(
            StorageKey::with_last_byte(3),
            StorageValue::from_str(
                "0x0000000000000000000000000000000000001db0000d27300000000000000005",
            )
            .unwrap(),
        );

        db.insert_account(L1_BLOCK_CONTRACT, l1_block_contract_account, None, l1_block_storage);

        db
    }

    fn executor_provider(chain_spec: Arc<ChainSpec>) -> OpExecutorProvider<OptimismEvmConfig> {
        let chain_spec = Arc::new(OpChainSpec::new(Arc::unwrap_or_clone(chain_spec)));
        OpExecutorProvider { evm_config: OptimismEvmConfig::new(chain_spec.clone()), chain_spec }
    }

    #[test]
    fn op_deposit_fields_pre_canyon() {
        let header = Header {
            timestamp: 1,
            number: 1,
            gas_limit: 1_000_000,
            gas_used: 42_000,
            receipts_root: b256!(
                "83465d1e7d01578c0d609be33570f91242f013e9e295b0879905346abbd63731"
            ),
            ..Default::default()
        };

        let mut db = create_op_state_provider();

        let addr = Address::ZERO;
        let account = Account { balance: U256::MAX, ..Account::default() };
        db.insert_account(addr, account, None, HashMap::default());

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&Arc::new(BASE_MAINNET.inner.clone()))
                .regolith_activated()
                .build(),
        );

        let tx = TransactionSigned::from_transaction_and_signature(
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: 0,
                gas_limit: MIN_TRANSACTION_GAS,
                to: addr.into(),
                ..Default::default()
            }),
            Signature::test_signature(),
        );

        let tx_deposit = TransactionSigned::from_transaction_and_signature(
            Transaction::Deposit(op_alloy_consensus::TxDeposit {
                from: addr,
                to: addr.into(),
                gas_limit: MIN_TRANSACTION_GAS,
                ..Default::default()
            }),
            Signature::test_signature(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        executor.state_mut().load_cache_account(L1_BLOCK_CONTRACT).unwrap();

        // Attempt to execute a block with one deposit and one non-deposit transaction
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: BlockBody {
                                transactions: vec![tx, tx_deposit],
                                ..Default::default()
                            },
                        },
                        senders: vec![addr, addr],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .unwrap();

        let tx_receipt = executor.receipts()[0][0].as_ref().unwrap();
        let deposit_receipt = executor.receipts()[0][1].as_ref().unwrap();

        // deposit_receipt_version is not present in pre canyon transactions
        assert!(deposit_receipt.deposit_receipt_version.is_none());
        assert!(tx_receipt.deposit_receipt_version.is_none());

        // deposit_nonce is present only in deposit transactions
        assert!(deposit_receipt.deposit_nonce.is_some());
        assert!(tx_receipt.deposit_nonce.is_none());
    }

    #[test]
    fn op_deposit_fields_post_canyon() {
        // ensure_create2_deployer will fail if timestamp is set to less then 2
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            gas_used: 42_000,
            receipts_root: b256!(
                "fffc85c4004fd03c7bfbe5491fae98a7473126c099ac11e8286fd0013f15f908"
            ),
            ..Default::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        let account = Account { balance: U256::MAX, ..Account::default() };

        db.insert_account(addr, account, None, HashMap::default());

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&Arc::new(BASE_MAINNET.inner.clone()))
                .canyon_activated()
                .build(),
        );

        let tx = TransactionSigned::from_transaction_and_signature(
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: 0,
                gas_limit: MIN_TRANSACTION_GAS,
                to: addr.into(),
                ..Default::default()
            }),
            Signature::test_signature(),
        );

        let tx_deposit = TransactionSigned::from_transaction_and_signature(
            Transaction::Deposit(op_alloy_consensus::TxDeposit {
                from: addr,
                to: addr.into(),
                gas_limit: MIN_TRANSACTION_GAS,
                ..Default::default()
            }),
            optimism_deposit_tx_signature(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        executor.state_mut().load_cache_account(L1_BLOCK_CONTRACT).unwrap();

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute_and_verify_one(
                (
                    &BlockWithSenders {
                        block: Block {
                            header,
                            body: BlockBody {
                                transactions: vec![tx, tx_deposit],
                                ..Default::default()
                            },
                        },
                        senders: vec![addr, addr],
                    },
                    U256::ZERO,
                )
                    .into(),
            )
            .expect("Executing a block while canyon is active should not fail");

        let tx_receipt = executor.receipts()[0][0].as_ref().unwrap();
        let deposit_receipt = executor.receipts()[0][1].as_ref().unwrap();

        // deposit_receipt_version is set to 1 for post canyon deposit transactions
        assert_eq!(deposit_receipt.deposit_receipt_version, Some(1));
        assert!(tx_receipt.deposit_receipt_version.is_none());

        // deposit_nonce is present only in deposit transactions
        assert!(deposit_receipt.deposit_nonce.is_some());
        assert!(tx_receipt.deposit_nonce.is_none());
    }

    #[test]
    fn test_initialisation() {
        // Create a new BundleState object with initial data
        let bundle = BundleState::new(
            vec![(Address::new([2; 20]), None, Some(AccountInfo::default()), HashMap::default())],
            vec![vec![(Address::new([2; 20]), None, vec![])]],
            vec![],
        );

        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![vec![Some(Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![],
                success: true,
                deposit_nonce: Some(18),
                deposit_receipt_version: Some(34),
            })]],
        };

        // Create a Requests object with a vector of requests, including DepositRequest and
        // WithdrawalRequest
        let requests = vec![Requests(vec![
            Request::DepositRequest(DepositRequest {
                pubkey: FixedBytes::<48>::from([1; 48]),
                withdrawal_credentials: B256::from([0; 32]),
                amount: 1111,
                signature: FixedBytes::<96>::from([2; 96]),
                index: 222,
            }),
            Request::DepositRequest(DepositRequest {
                pubkey: FixedBytes::<48>::from([23; 48]),
                withdrawal_credentials: B256::from([0; 32]),
                amount: 34343,
                signature: FixedBytes::<96>::from([43; 96]),
                index: 1212,
            }),
            Request::WithdrawalRequest(WithdrawalRequest {
                source_address: Address::from([1; 20]),
                validator_pubkey: FixedBytes::<48>::from([10; 48]),
                amount: 72,
            }),
        ])];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: bundle.clone(),
            receipts: receipts.clone(),
            requests: requests.clone(),
            first_block,
        };

        // Assert that creating a new ExecutionOutcome using the constructor matches exec_res
        assert_eq!(
            ExecutionOutcome::new(bundle, receipts.clone(), first_block, requests.clone()),
            exec_res
        );

        // Create a BundleStateInit object and insert initial data
        let mut state_init: BundleStateInit = HashMap::default();
        state_init
            .insert(Address::new([2; 20]), (None, Some(Account::default()), HashMap::default()));

        // Create a HashMap for account reverts and insert initial data
        let mut revert_inner: HashMap<Address, AccountRevertInit> = HashMap::default();
        revert_inner.insert(Address::new([2; 20]), (None, vec![]));

        // Create a RevertsInit object and insert the revert_inner data
        let mut revert_init: RevertsInit = HashMap::default();
        revert_init.insert(123, revert_inner);

        // Assert that creating a new ExecutionOutcome using the new_init method matches
        // exec_res
        assert_eq!(
            ExecutionOutcome::new_init(
                state_init,
                revert_init,
                vec![],
                receipts,
                first_block,
                requests,
            ),
            exec_res
        );
    }

    #[test]
    fn test_block_number_to_index() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![vec![Some(Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![],
                success: true,
                deposit_nonce: Some(18),
                deposit_receipt_version: Some(34),
            })]],
        };

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(),
            receipts,
            requests: vec![],
            first_block,
        };

        // Test before the first block
        assert_eq!(exec_res.block_number_to_index(12), None);

        // Test after after the first block but index larger than receipts length
        assert_eq!(exec_res.block_number_to_index(133), None);

        // Test after the first block
        assert_eq!(exec_res.block_number_to_index(123), Some(0));
    }

    #[test]
    fn test_get_logs() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![vec![Some(Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![Log::<LogData>::default()],
                success: true,
                deposit_nonce: Some(18),
                deposit_receipt_version: Some(34),
            })]],
        };

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(),
            receipts,
            requests: vec![],
            first_block,
        };

        // Get logs for block number 123
        let logs: Vec<&Log> = exec_res.logs(123).unwrap().collect();

        // Assert that the logs match the expected logs
        assert_eq!(logs, vec![&Log::<LogData>::default()]);
    }

    #[test]
    fn test_receipts_by_block() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![vec![Some(Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![Log::<LogData>::default()],
                success: true,
                deposit_nonce: Some(18),
                deposit_receipt_version: Some(34),
            })]],
        };

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(), // Default value for bundle
            receipts,                   // Include the created receipts
            requests: vec![],           // Empty vector for requests
            first_block,                // Set the first block number
        };

        // Get receipts for block number 123 and convert the result into a vector
        let receipts_by_block: Vec<_> = exec_res.receipts_by_block(123).iter().collect();

        // Assert that the receipts for block number 123 match the expected receipts
        assert_eq!(
            receipts_by_block,
            vec![&Some(Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![Log::<LogData>::default()],
                success: true,
                deposit_nonce: Some(18),
                deposit_receipt_version: Some(34),
            })]
        );
    }

    #[test]
    fn test_receipts_len() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![vec![Some(Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![Log::<LogData>::default()],
                success: true,
                deposit_nonce: Some(18),
                deposit_receipt_version: Some(34),
            })]],
        };

        // Create an empty Receipts object
        let receipts_empty = Receipts { receipt_vec: vec![] };

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(), // Default value for bundle
            receipts,                   // Include the created receipts
            requests: vec![],           // Empty vector for requests
            first_block,                // Set the first block number
        };

        // Assert that the length of receipts in exec_res is 1
        assert_eq!(exec_res.len(), 1);

        // Assert that exec_res is not empty
        assert!(!exec_res.is_empty());

        // Create a ExecutionOutcome object with an empty Receipts object
        let exec_res_empty_receipts = ExecutionOutcome {
            bundle: Default::default(), // Default value for bundle
            receipts: receipts_empty,   // Include the empty receipts
            requests: vec![],           // Empty vector for requests
            first_block,                // Set the first block number
        };

        // Assert that the length of receipts in exec_res_empty_receipts is 0
        assert_eq!(exec_res_empty_receipts.len(), 0);

        // Assert that exec_res_empty_receipts is empty
        assert!(exec_res_empty_receipts.is_empty());
    }

    #[test]
    fn test_revert_to() {
        // Create a random receipt object
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
            deposit_nonce: Some(18),
            deposit_receipt_version: Some(34),
        };

        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![vec![Some(receipt.clone())], vec![Some(receipt.clone())]],
        };

        // Define the first block number
        let first_block = 123;

        // Create a DepositRequest object with specific attributes.
        let request = Request::DepositRequest(DepositRequest {
            pubkey: FixedBytes::<48>::from([1; 48]),
            withdrawal_credentials: B256::from([0; 32]),
            amount: 1111,
            signature: FixedBytes::<96>::from([2; 96]),
            index: 222,
        });

        // Create a vector of Requests containing the request.
        let requests = vec![Requests(vec![request]), Requests(vec![request])];

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let mut exec_res =
            ExecutionOutcome { bundle: Default::default(), receipts, requests, first_block };

        // Assert that the revert_to method returns true when reverting to the initial block number.
        assert!(exec_res.revert_to(123));

        // Assert that the receipts are properly cut after reverting to the initial block number.
        assert_eq!(exec_res.receipts, Receipts { receipt_vec: vec![vec![Some(receipt)]] });

        // Assert that the requests are properly cut after reverting to the initial block number.
        assert_eq!(exec_res.requests, vec![Requests(vec![request])]);

        // Assert that the revert_to method returns false when attempting to revert to a block
        // number greater than the initial block number.
        assert!(!exec_res.revert_to(133));

        // Assert that the revert_to method returns false when attempting to revert to a block
        // number less than the initial block number.
        assert!(!exec_res.revert_to(10));
    }

    #[test]
    fn test_extend_execution_outcome() {
        // Create a Receipt object with specific attributes.
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
            deposit_nonce: Some(18),
            deposit_receipt_version: Some(34),
        };

        // Create a Receipts object containing the receipt.
        let receipts = Receipts { receipt_vec: vec![vec![Some(receipt.clone())]] };

        // Create a DepositRequest object with specific attributes.
        let request = Request::DepositRequest(DepositRequest {
            pubkey: FixedBytes::<48>::from([1; 48]),
            withdrawal_credentials: B256::from([0; 32]),
            amount: 1111,
            signature: FixedBytes::<96>::from([2; 96]),
            index: 222,
        });

        // Create a vector of Requests containing the request.
        let requests = vec![Requests(vec![request])];

        // Define the initial block number.
        let first_block = 123;

        // Create an ExecutionOutcome object.
        let mut exec_res =
            ExecutionOutcome { bundle: Default::default(), receipts, requests, first_block };

        // Extend the ExecutionOutcome object by itself.
        exec_res.extend(exec_res.clone());

        // Assert the extended ExecutionOutcome matches the expected outcome.
        assert_eq!(
            exec_res,
            ExecutionOutcome {
                bundle: Default::default(),
                receipts: Receipts {
                    receipt_vec: vec![vec![Some(receipt.clone())], vec![Some(receipt)]]
                },
                requests: vec![Requests(vec![request]), Requests(vec![request])],
                first_block: 123,
            }
        );
    }

    #[test]
    fn test_split_at_execution_outcome() {
        // Create a random receipt object
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
            deposit_nonce: Some(18),
            deposit_receipt_version: Some(34),
        };

        // Create a Receipts object with a vector of receipt vectors
        let receipts = Receipts {
            receipt_vec: vec![
                vec![Some(receipt.clone())],
                vec![Some(receipt.clone())],
                vec![Some(receipt.clone())],
            ],
        };

        // Define the first block number
        let first_block = 123;

        // Create a DepositRequest object with specific attributes.
        let request = Request::DepositRequest(DepositRequest {
            pubkey: FixedBytes::<48>::from([1; 48]),
            withdrawal_credentials: B256::from([0; 32]),
            amount: 1111,
            signature: FixedBytes::<96>::from([2; 96]),
            index: 222,
        });

        // Create a vector of Requests containing the request.
        let requests =
            vec![Requests(vec![request]), Requests(vec![request]), Requests(vec![request])];

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res =
            ExecutionOutcome { bundle: Default::default(), receipts, requests, first_block };

        // Split the ExecutionOutcome at block number 124
        let result = exec_res.clone().split_at(124);

        // Define the expected lower ExecutionOutcome after splitting
        let lower_execution_outcome = ExecutionOutcome {
            bundle: Default::default(),
            receipts: Receipts { receipt_vec: vec![vec![Some(receipt.clone())]] },
            requests: vec![Requests(vec![request])],
            first_block,
        };

        // Define the expected higher ExecutionOutcome after splitting
        let higher_execution_outcome = ExecutionOutcome {
            bundle: Default::default(),
            receipts: Receipts {
                receipt_vec: vec![vec![Some(receipt.clone())], vec![Some(receipt)]],
            },
            requests: vec![Requests(vec![request]), Requests(vec![request])],
            first_block: 124,
        };

        // Assert that the split result matches the expected lower and higher outcomes
        assert_eq!(result.0, Some(lower_execution_outcome));
        assert_eq!(result.1, higher_execution_outcome);

        // Assert that splitting at the first block number returns None for the lower outcome
        assert_eq!(exec_res.clone().split_at(123), (None, exec_res));
    }
}
