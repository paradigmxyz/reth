//! Optimism block execution strategy.

use crate::{
    l1::ensure_create2_deployer, BasicOpReceiptBuilder, OpBlockExecutionError, OpEvmConfig,
    OpReceiptBuilder, ReceiptBuilderCtx,
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_consensus::{
    transaction::Recovered, BlockHeader, Eip658Value, Header, Receipt, Transaction as _, TxReceipt,
};
use alloy_evm::FromRecoveredTx;
use op_alloy_consensus::OpDepositReceipt;
use reth_evm::{
    execute::{
        balance_increment_state, BasicBlockExecutorProvider, BlockExecutionError,
        BlockExecutionStrategy, BlockExecutionStrategyFactory, BlockValidationError,
    },
    state_change::post_block_balance_increments,
    system_calls::{OnStateHook, StateChangePostBlockSource, StateChangeSource, SystemCaller},
    ConfigureEvm, ConfigureEvmFor, Database, Evm, HaltReasonFor,
};
use reth_execution_types::BlockExecutionResult;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::{transaction::signed::OpTransaction, DepositReceipt, OpPrimitives};
use reth_primitives_traits::{
    Block, NodePrimitives, RecoveredBlock, SealedBlock, SignedTransaction,
};
use revm::{context_interface::result::ResultAndState, DatabaseCommit};
use revm_database::State;
use revm_primitives::{Address, B256};
use tracing::trace;

/// Factory for [`OpExecutionStrategy`].
#[derive(Debug, Clone)]
pub struct OpExecutionStrategyFactory<
    N: NodePrimitives = OpPrimitives,
    ChainSpec = OpChainSpec,
    EvmConfig: ConfigureEvm = OpEvmConfig<ChainSpec>,
> {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: EvmConfig,
    /// Receipt builder.
    receipt_builder:
        Arc<dyn OpReceiptBuilder<N::SignedTx, HaltReasonFor<EvmConfig>, Receipt = N::Receipt>>,
}

impl OpExecutionStrategyFactory<OpPrimitives> {
    /// Creates a new default optimism executor strategy factory.
    pub fn optimism(chain_spec: Arc<OpChainSpec>) -> Self {
        Self::new(
            chain_spec.clone(),
            OpEvmConfig::new(chain_spec),
            BasicOpReceiptBuilder::default(),
        )
    }
}

impl<N: NodePrimitives, ChainSpec, EvmConfig: ConfigureEvm>
    OpExecutionStrategyFactory<N, ChainSpec, EvmConfig>
{
    /// Creates a new executor strategy factory.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        evm_config: EvmConfig,
        receipt_builder: impl OpReceiptBuilder<
            N::SignedTx,
            HaltReasonFor<EvmConfig>,
            Receipt = N::Receipt,
        >,
    ) -> Self {
        Self { chain_spec, evm_config, receipt_builder: Arc::new(receipt_builder) }
    }
}

impl<N, ChainSpec, EvmConfig> BlockExecutionStrategyFactory
    for OpExecutionStrategyFactory<N, ChainSpec, EvmConfig>
where
    N: NodePrimitives<SignedTx: OpTransaction, Receipt: DepositReceipt>,
    ChainSpec: OpHardforks + Clone + Unpin + Sync + Send + 'static,
    EvmConfig: ConfigureEvmFor<N> + Clone + Unpin + Sync + Send + 'static,
{
    type Primitives = N;

    fn create_strategy<'a, DB>(
        &'a mut self,
        db: &'a mut State<DB>,
        block: &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> impl BlockExecutionStrategy<Primitives = Self::Primitives, Error = BlockExecutionError> + 'a
    where
        DB: Database,
    {
        let evm = self.evm_config.evm_for_block(db, block.header());
        OpExecutionStrategy::new(
            evm,
            block.sealed_block(),
            &self.chain_spec,
            self.receipt_builder.as_ref(),
        )
    }
}

/// Input for block execution.
#[derive(Debug, Clone, Copy)]
pub struct OpBlockExecutionInput {
    /// Block number.
    pub number: u64,
    /// Block timestamp.
    pub timestamp: u64,
    /// Parent block hash.
    pub parent_hash: B256,
    /// Block gas limit.
    pub gas_limit: u64,
    /// Parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// Block beneficiary.
    pub beneficiary: Address,
}

impl<'a, B: Block> From<&'a SealedBlock<B>> for OpBlockExecutionInput {
    fn from(block: &'a SealedBlock<B>) -> Self {
        Self {
            number: block.header().number(),
            timestamp: block.header().timestamp(),
            parent_hash: block.header().parent_hash(),
            gas_limit: block.header().gas_limit(),
            parent_beacon_block_root: block.header().parent_beacon_block_root(),
            beneficiary: block.header().beneficiary(),
        }
    }
}

/// Block execution strategy for Optimism.
#[derive(Debug)]
pub struct OpExecutionStrategy<'a, E: Evm, N: NodePrimitives, ChainSpec> {
    /// Chainspec.
    chain_spec: ChainSpec,
    /// Receipt builder.
    receipt_builder: &'a dyn OpReceiptBuilder<N::SignedTx, E::HaltReason, Receipt = N::Receipt>,

    /// Input for block execution.
    input: OpBlockExecutionInput,
    /// The EVM used by strategy.
    evm: E,
    /// Receipts of executed transactions.
    receipts: Vec<N::Receipt>,
    /// Total gas used by executed transactions.
    gas_used: u64,
    /// Whether Regolith hardfork is active.
    is_regolith: bool,
    /// Utility to call system smart contracts.
    system_caller: SystemCaller<ChainSpec>,
}

impl<'a, E, N, ChainSpec> OpExecutionStrategy<'a, E, N, ChainSpec>
where
    E: Evm,
    N: NodePrimitives,
    ChainSpec: OpHardforks,
{
    /// Creates a new [`OpExecutionStrategy`]
    pub fn new(
        evm: E,
        input: impl Into<OpBlockExecutionInput>,
        chain_spec: ChainSpec,
        receipt_builder: &'a dyn OpReceiptBuilder<N::SignedTx, E::HaltReason, Receipt = N::Receipt>,
    ) -> Self {
        let input = input.into();
        Self {
            is_regolith: chain_spec.is_regolith_active_at_timestamp(input.timestamp),
            evm,
            system_caller: SystemCaller::new(chain_spec.clone()),
            chain_spec,
            receipt_builder,
            receipts: Vec::new(),
            gas_used: 0,
            input,
        }
    }
}

impl<'db, DB, E, N, ChainSpec> BlockExecutionStrategy for OpExecutionStrategy<'_, E, N, ChainSpec>
where
    DB: Database + 'db,
    E: Evm<DB = &'db mut State<DB>, Tx: FromRecoveredTx<N::SignedTx>>,
    N: NodePrimitives<SignedTx: OpTransaction, Receipt: DepositReceipt>,
    ChainSpec: OpHardforks,
{
    type Primitives = N;
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error> {
        // Set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            self.chain_spec.is_spurious_dragon_active_at_block(self.input.number);
        self.evm.db_mut().set_state_clear_flag(state_clear_flag);

        self.system_caller
            .apply_beacon_root_contract_call(self.input.parent_beacon_block_root, &mut self.evm)?;

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        ensure_create2_deployer(self.chain_spec.clone(), self.input.timestamp, self.evm.db_mut())
            .map_err(|_| OpBlockExecutionError::ForceCreate2DeployerFail)?;

        Ok(())
    }

    fn execute_transaction(&mut self, tx: Recovered<&N::SignedTx>) -> Result<u64, Self::Error> {
        // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = self.input.gas_limit - self.gas_used;
        if tx.gas_limit() > block_available_gas && (self.is_regolith || !tx.is_deposit()) {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx.gas_limit(),
                block_available_gas,
            }
            .into())
        }

        // Cache the depositor account prior to the state transition for the deposit nonce.
        //
        // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
        // were not introduced in Bedrock. In addition, regular transactions don't have deposit
        // nonces, so we don't need to touch the DB for those.
        let depositor = (self.is_regolith && tx.is_deposit())
            .then(|| {
                self.evm
                    .db_mut()
                    .load_cache_account(tx.signer())
                    .map(|acc| acc.account_info().unwrap_or_default())
            })
            .transpose()
            .map_err(|_| OpBlockExecutionError::AccountLoadFailed(tx.signer()))?;

        let hash = tx.tx_hash();

        // Execute transaction.
        let result_and_state =
            self.evm.transact(&tx).map_err(move |err| BlockExecutionError::evm(err, *hash))?;

        trace!(
            target: "evm",
            ?tx,
            "Executed transaction"
        );
        self.system_caller
            .on_state(StateChangeSource::Transaction(self.receipts.len()), &result_and_state.state);
        let ResultAndState { result, state } = result_and_state;
        self.evm.db_mut().commit(state);

        let gas_used = result.gas_used();

        // append gas used
        self.gas_used += gas_used;

        self.receipts.push(
            match self.receipt_builder.build_receipt(ReceiptBuilderCtx {
                tx: tx.tx(),
                result,
                cumulative_gas_used: self.gas_used,
            }) {
                Ok(receipt) => receipt,
                Err(ctx) => {
                    let receipt = Receipt {
                        // Success flag was added in `EIP-658: Embedding transaction status code
                        // in receipts`.
                        status: Eip658Value::Eip658(ctx.result.is_success()),
                        cumulative_gas_used: self.gas_used,
                        logs: ctx.result.into_logs(),
                    };

                    self.receipt_builder.build_deposit_receipt(OpDepositReceipt {
                        inner: receipt,
                        deposit_nonce: depositor.map(|account| account.nonce),
                        // The deposit receipt version was introduced in Canyon to indicate an
                        // update to how receipt hashes should be computed
                        // when set. The state transition process ensures
                        // this is only set for post-Canyon deposit
                        // transactions.
                        deposit_receipt_version: (tx.is_deposit() &&
                            self.chain_spec.is_canyon_active_at_timestamp(self.input.timestamp))
                        .then_some(1),
                    })
                }
            },
        );

        Ok(gas_used)
    }

    fn apply_post_execution_changes(
        mut self,
    ) -> Result<BlockExecutionResult<N::Receipt>, Self::Error> {
        let balance_increments = post_block_balance_increments::<Header>(
            &self.chain_spec.clone(),
            self.evm.block(),
            &[],
            None,
        );
        // increment balances
        self.evm
            .db_mut()
            .increment_balances(balance_increments.clone())
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;
        // call state hook with changes due to balance increments.
        let balance_state = balance_increment_state(&balance_increments, self.evm.db_mut())?;
        self.system_caller.on_state(
            StateChangeSource::PostBlock(StateChangePostBlockSource::BalanceIncrements),
            &balance_state,
        );

        let gas_used = self.receipts.last().map(|r| r.cumulative_gas_used()).unwrap_or_default();
        Ok(BlockExecutionResult { receipts: self.receipts, requests: Default::default(), gas_used })
    }

    fn with_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.system_caller.with_state_hook(hook);
    }
}

/// Helper type with backwards compatible methods to obtain executor providers.
#[derive(Debug)]
pub struct OpExecutorProvider;

impl OpExecutorProvider {
    /// Creates a new default optimism executor strategy factory.
    pub fn optimism(
        chain_spec: Arc<OpChainSpec>,
    ) -> BasicBlockExecutorProvider<OpExecutionStrategyFactory<OpPrimitives>> {
        BasicBlockExecutorProvider::new(OpExecutionStrategyFactory::optimism(chain_spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpChainSpec;
    use alloy_consensus::{Block, BlockBody, Header, TxEip1559};
    use alloy_primitives::{
        b256, Address, PrimitiveSignature as Signature, StorageKey, StorageValue, U256,
    };
    use op_alloy_consensus::{OpTypedTransaction, TxDeposit};
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_evm::execute::{BasicBlockExecutorProvider, BlockExecutorProvider, Executor};
    use reth_optimism_chainspec::OpChainSpecBuilder;
    use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
    use reth_primitives_traits::Account;
    use reth_revm::{database::StateProviderDatabase, test_utils::StateProviderTest};
    use revm_optimism::constants::L1_BLOCK_CONTRACT;
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

    fn executor_provider(
        chain_spec: Arc<OpChainSpec>,
    ) -> BasicBlockExecutorProvider<OpExecutionStrategyFactory> {
        let strategy_factory = OpExecutionStrategyFactory::optimism(chain_spec);

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

        let tx = OpTransactionSigned::new_unhashed(
            OpTypedTransaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: 0,
                gas_limit: MIN_TRANSACTION_GAS,
                to: addr.into(),
                ..Default::default()
            }),
            Signature::test_signature(),
        );

        let tx_deposit = OpTransactionSigned::new_unhashed(
            OpTypedTransaction::Deposit(op_alloy_consensus::TxDeposit {
                from: addr,
                to: addr.into(),
                gas_limit: MIN_TRANSACTION_GAS,
                ..Default::default()
            }),
            Signature::test_signature(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.executor(StateProviderDatabase::new(&db));

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        // Attempt to execute a block with one deposit and one non-deposit transaction
        let output = executor
            .execute(&RecoveredBlock::new_unhashed(
                Block {
                    header,
                    body: BlockBody { transactions: vec![tx, tx_deposit], ..Default::default() },
                },
                vec![addr, addr],
            ))
            .unwrap();

        let receipts = &output.receipts;
        let tx_receipt = &receipts[0];
        let deposit_receipt = &receipts[1];

        assert!(!matches!(tx_receipt, OpReceipt::Deposit(_)));
        // deposit_nonce is present only in deposit transactions
        let OpReceipt::Deposit(deposit_receipt) = deposit_receipt else {
            panic!("expected deposit")
        };
        assert!(deposit_receipt.deposit_nonce.is_some());
        // deposit_receipt_version is not present in pre canyon transactions
        assert!(deposit_receipt.deposit_receipt_version.is_none());
    }

    #[test]
    fn op_deposit_fields_post_canyon() {
        // ensure_create2_deployer will fail if timestamp is set to less than 2
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

        let tx = OpTransactionSigned::new_unhashed(
            OpTypedTransaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: 0,
                gas_limit: MIN_TRANSACTION_GAS,
                to: addr.into(),
                ..Default::default()
            }),
            Signature::test_signature(),
        );

        let tx_deposit = OpTransactionSigned::new_unhashed(
            OpTypedTransaction::Deposit(op_alloy_consensus::TxDeposit {
                from: addr,
                to: addr.into(),
                gas_limit: MIN_TRANSACTION_GAS,
                ..Default::default()
            }),
            TxDeposit::signature(),
        );

        let provider = executor_provider(chain_spec);
        let mut executor = provider.executor(StateProviderDatabase::new(&db));

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        // attempt to execute an empty block with parent beacon block root, this should not fail
        let output = executor
            .execute(&RecoveredBlock::new_unhashed(
                Block {
                    header,
                    body: BlockBody { transactions: vec![tx, tx_deposit], ..Default::default() },
                },
                vec![addr, addr],
            ))
            .expect("Executing a block while canyon is active should not fail");

        let receipts = &output.receipts;
        let tx_receipt = &receipts[0];
        let deposit_receipt = &receipts[1];

        // deposit_receipt_version is set to 1 for post canyon deposit transactions
        assert!(!matches!(tx_receipt, OpReceipt::Deposit(_)));
        let OpReceipt::Deposit(deposit_receipt) = deposit_receipt else {
            panic!("expected deposit")
        };
        assert_eq!(deposit_receipt.deposit_receipt_version, Some(1));

        // deposit_nonce is present only in deposit transactions
        assert!(deposit_receipt.deposit_nonce.is_some());
    }
}
