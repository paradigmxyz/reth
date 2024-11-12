//! Optimism block execution strategy.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::fmt::Display;

use alloy_consensus::Transaction as _;
use alloy_eips::eip7685::Requests;
use op_alloy_consensus::DepositTransaction;
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_evm::{
    execute::{
        BasicBlockExecutorProvider, BlockExecutionError, BlockExecutionStrategy,
        BlockExecutionStrategyFactory, BlockValidationError, ExecuteOutput, ProviderError,
    },
    state_change::post_block_balance_increments,
    system_calls::{OnStateHook, SystemCaller},
    ConfigureEvm,
};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::validate_block_post_execution;

use crate::{l1::ensure_create2_deployer, OpBlockExecutionError, OpEvmConfig};

/// Factory for [`OpExecutionStrategy`].
#[derive(Debug, Clone)]
pub struct OpExecutionStrategyFactory<EvmConfig = OpEvmConfig> {
    /// The chainspec
    chain_spec: Arc<OpChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
}

impl OpExecutionStrategyFactory {
    /// Creates a new default optimism executor strategy factory.
    pub fn optimism(chain_spec: Arc<OpChainSpec>) -> Self {
        Self::new(chain_spec.clone(), OpEvmConfig::new(chain_spec))
    }
}

impl<EvmConfig> OpExecutionStrategyFactory<EvmConfig> {
    /// Creates a new executor strategy factory.
    pub const fn new(chain_spec: Arc<OpChainSpec>, evm_config: EvmConfig) -> Self {
        Self { chain_spec, evm_config }
    }
}

impl<EvmConfig> BlockExecutionStrategyFactory for OpExecutionStrategyFactory<EvmConfig>
where
    EvmConfig:
        Clone + Unpin + Sync + Send + 'static + ConfigureEvm<Header = alloy_consensus::Header>,
{
    type Strategy<DB: Database<Error: Into<ProviderError> + Display>> =
        OpExecutionStrategy<DB, EvmConfig>;

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
pub struct OpExecutionStrategy<DB, EvmConfig>
where
    EvmConfig: Clone,
{
    /// The chainspec
    chain_spec: Arc<OpChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
    /// Current state for block execution.
    state: State<DB>,
    /// Utility to call system smart contracts.
    system_caller: SystemCaller<EvmConfig, OpChainSpec>,
}

impl<DB, EvmConfig> OpExecutionStrategy<DB, EvmConfig>
where
    EvmConfig: Clone,
{
    /// Creates a new [`OpExecutionStrategy`]
    pub fn new(state: State<DB>, chain_spec: Arc<OpChainSpec>, evm_config: EvmConfig) -> Self {
        let system_caller = SystemCaller::new(evm_config.clone(), chain_spec.clone());
        Self { state, chain_spec, evm_config, system_caller }
    }
}

impl<DB, EvmConfig> OpExecutionStrategy<DB, EvmConfig>
where
    DB: Database<Error: Into<ProviderError> + Display>,
    EvmConfig: ConfigureEvm<Header = alloy_consensus::Header>,
{
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

impl<DB, EvmConfig> BlockExecutionStrategy<DB> for OpExecutionStrategy<DB, EvmConfig>
where
    DB: Database<Error: Into<ProviderError> + Display>,
    EvmConfig: ConfigureEvm<Header = alloy_consensus::Header>,
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
            .map_err(|_| OpBlockExecutionError::ForceCreate2DeployerFail)?;

        Ok(())
    }

    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<ExecuteOutput, Self::Error> {
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);

        let is_regolith =
            self.chain_spec.fork(OpHardfork::Regolith).active_at_timestamp(block.timestamp);

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
                return Err(OpBlockExecutionError::BlobTransactionRejected.into())
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
                .map_err(|_| OpBlockExecutionError::AccountLoadFailed(*sender))?;

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
                        .is_fork_active_at_timestamp(OpHardfork::Canyon, block.timestamp))
                .then_some(1),
            });
        }

        Ok(ExecuteOutput { receipts, gas_used: cumulative_gas_used })
    }

    fn apply_post_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        _receipts: &[Receipt],
    ) -> Result<Requests, Self::Error> {
        let balance_increments =
            post_block_balance_increments(&self.chain_spec.clone(), block, total_difficulty);
        // increment balances
        self.state
            .increment_balances(balance_increments)
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

        Ok(Requests::default())
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

    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders,
        receipts: &[Receipt],
        _requests: &Requests,
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec.clone(), receipts)
    }
}

/// Helper type with backwards compatible methods to obtain executor providers.
#[derive(Debug)]
pub struct OpExecutorProvider;

impl OpExecutorProvider {
    /// Creates a new default optimism executor strategy factory.
    pub fn optimism(
        chain_spec: Arc<OpChainSpec>,
    ) -> BasicBlockExecutorProvider<OpExecutionStrategyFactory> {
        BasicBlockExecutorProvider::new(OpExecutionStrategyFactory::optimism(chain_spec))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, str::FromStr};

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

    use super::*;
    use crate::OpChainSpec;

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

    fn executor_provider(
        chain_spec: Arc<OpChainSpec>,
    ) -> BasicBlockExecutorProvider<OpExecutionStrategyFactory> {
        let strategy_factory =
            OpExecutionStrategyFactory::new(chain_spec.clone(), OpEvmConfig::new(chain_spec));

        BasicBlockExecutorProvider::new(strategy_factory)
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

        let chain_spec = Arc::new(OpChainSpecBuilder::base_mainnet().regolith_activated().build());

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

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

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

        let receipts = executor.receipts();
        let tx_receipt = receipts[0][0].as_ref().unwrap();
        let deposit_receipt = receipts[0][1].as_ref().unwrap();

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

        let chain_spec = Arc::new(OpChainSpecBuilder::base_mainnet().canyon_activated().build());

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
            TxDeposit::signature(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.batch_executor(StateProviderDatabase::new(&db));

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

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

        let receipts = executor.receipts();
        let tx_receipt = receipts[0][0].as_ref().unwrap();
        let deposit_receipt = receipts[0][1].as_ref().unwrap();

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
