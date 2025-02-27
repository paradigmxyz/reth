//! Implementation of the [`BlockExecutionStrategy`] for Scroll.

use crate::{
    config::{ScrollConfigureEvm, ScrollEvmT},
    receipt::{BasicScrollReceiptBuilder, ReceiptBuilderCtx, ScrollReceiptBuilder},
    ForkError, ScrollBlockExecutionError, ScrollEvmConfig,
};
use alloy_consensus::{transaction::Recovered, BlockHeader, Header, Transaction, Typed2718};
use alloy_evm::FromRecoveredTx;
use reth_consensus::ConsensusError;
use reth_evm::{
    execute::{
        BasicBlockExecutorProvider, BlockExecutionError, BlockExecutionStrategy,
        BlockExecutionStrategyFactory, BlockValidationError,
    },
    system_calls::OnStateHook,
    ConfigureEvm, Database, Evm, HaltReasonFor,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives::{InvalidTransactionError, SealedBlock};
use reth_primitives_traits::{Block, NodePrimitives, RecoveredBlock, SignedTransaction};
use reth_revm::{
    precompile::{Address, B256},
    primitives::U256,
};
use reth_scroll_chainspec::{ChainConfig, ChainSpecProvider, ScrollChainConfig, ScrollChainSpec};
use reth_scroll_consensus::{apply_curie_hard_fork, L1_GAS_PRICE_ORACLE_ADDRESS};
use reth_scroll_forks::{ScrollHardfork, ScrollHardforks};
use reth_scroll_primitives::{transaction::signed::IsL1Message, ScrollPrimitives, ScrollReceipt};
use revm::{context::result::ResultAndState, database::State, DatabaseCommit};
use std::{fmt::Debug, marker::PhantomData, sync::Arc};
use tracing::trace;

/// The factory for a [`ScrollExecutionStrategy`].
#[derive(Clone, Debug)]
pub struct ScrollExecutionStrategyFactory<
    N: NodePrimitives = ScrollPrimitives,
    ChainSpec = ScrollChainSpec,
    EvmConfig: ConfigureEvm = ScrollEvmConfig<ChainSpec>,
> {
    /// The Evm configuration for the [`ScrollExecutionStrategy`].
    evm_config: EvmConfig,
    /// Receipt builder.
    receipt_builder:
        Arc<dyn ScrollReceiptBuilder<N::SignedTx, HaltReasonFor<EvmConfig>, Receipt = N::Receipt>>,
    /// Chain specification marker
    _chain_spec: PhantomData<ChainSpec>,
}

impl ScrollExecutionStrategyFactory {
    /// Returns a default Scroll factory.
    pub fn scroll(chain_spec: Arc<ScrollChainSpec>) -> Self {
        Self::new(ScrollEvmConfig::new(chain_spec), BasicScrollReceiptBuilder::default())
    }
}

impl<N: NodePrimitives, ChainSpec, EvmConfig>
    ScrollExecutionStrategyFactory<N, ChainSpec, EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    /// Returns a new instance of the [`ScrollExecutionStrategyFactory`].
    pub fn new(
        evm_config: EvmConfig,
        receipt_builder: impl ScrollReceiptBuilder<
            N::SignedTx,
            HaltReasonFor<EvmConfig>,
            Receipt = N::Receipt,
        >,
    ) -> Self {
        Self { evm_config, receipt_builder: Arc::new(receipt_builder), _chain_spec: PhantomData }
    }

    /// Returns the EVM configuration for the strategy factory.
    pub fn evm_config(&self) -> EvmConfig {
        self.evm_config.clone()
    }
}

impl<N, ChainSpec, EvmConfig> BlockExecutionStrategyFactory
    for ScrollExecutionStrategyFactory<N, ChainSpec, EvmConfig>
where
    N: NodePrimitives<BlockHeader = Header, Receipt = ScrollReceipt, SignedTx: IsL1Message>,
    ChainSpec: ScrollHardforks
        + ChainConfig<Config = ScrollChainConfig>
        + Clone
        + Unpin
        + Sync
        + Send
        + 'static,
    EvmConfig: ScrollConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + ChainSpecProvider<ChainSpec = ScrollChainSpec>
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static,
{
    type Primitives = N;

    /// Creates a strategy using the give database.
    fn create_strategy<'a, DB>(
        &'a mut self,
        db: &'a mut State<DB>,
        block: &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> impl BlockExecutionStrategy<Primitives = Self::Primitives, Error = BlockExecutionError> + 'a
    where
        DB: Database,
    {
        let evm = self.evm_config.scroll_evm_for_block(db, block.header());
        ScrollExecutionStrategy::new(
            evm,
            block.sealed_block(),
            self.evm_config.chain_spec(),
            self.receipt_builder.as_ref(),
        )
    }
}

/// Input for block execution.
#[derive(Debug, Clone, Copy)]
pub struct ScrollBlockExecutionInput {
    /// Block number.
    pub number: u64,
    /// Block timestamp.
    pub timestamp: u64,
    /// Parent block hash.
    pub parent_hash: B256,
    /// Block gas limit.
    pub gas_limit: u64,
    /// Block beneficiary.
    pub beneficiary: Address,
}

impl<B: Block> From<&SealedBlock<B>> for ScrollBlockExecutionInput {
    fn from(block: &SealedBlock<B>) -> Self {
        Self {
            number: block.header().number(),
            timestamp: block.header().timestamp(),
            parent_hash: block.header().parent_hash(),
            gas_limit: block.header().gas_limit(),
            beneficiary: block.header().beneficiary(),
        }
    }
}

/// The Scroll block execution strategy.
#[derive(Debug)]
pub struct ScrollExecutionStrategy<'a, E: Evm, N: NodePrimitives, ChainSpec> {
    /// The chain specification.
    chain_spec: ChainSpec,
    /// Input to the execution.
    input: ScrollBlockExecutionInput,
    /// The EVM instance used for execution.
    evm: E,
    /// The receipts resulting from the block's execution.
    receipts: Vec<N::Receipt>,
    /// The gas used by the transactions.
    gas_used: u64,
    /// Receipt builder.
    receipt_builder: &'a dyn ScrollReceiptBuilder<N::SignedTx, E::HaltReason, Receipt = N::Receipt>,
}

impl<'a, E, N, ChainSpec> ScrollExecutionStrategy<'a, E, N, ChainSpec>
where
    N: NodePrimitives,
    E: Evm,
    ChainSpec: ScrollHardforks,
{
    /// Returns an instance of [`ScrollExecutionStrategy`].
    pub fn new(
        evm: E,
        input: impl Into<ScrollBlockExecutionInput>,
        chain_spec: ChainSpec,
        receipt_builder: &'a dyn ScrollReceiptBuilder<
            N::SignedTx,
            E::HaltReason,
            Receipt = N::Receipt,
        >,
    ) -> Self {
        let input = input.into();
        Self { evm, input, chain_spec, receipts: vec![], gas_used: 0, receipt_builder }
    }
}

impl<'db, DB, E, N, ChainSpec> BlockExecutionStrategy
    for ScrollExecutionStrategy<'_, E, N, ChainSpec>
where
    DB: Database + 'db,
    E: ScrollEvmT<DB = &'db mut State<DB>, Tx: FromRecoveredTx<N::SignedTx>>,
    N: NodePrimitives<Receipt = ScrollReceipt, SignedTx: IsL1Message>,
    ChainSpec: ScrollHardforks,
{
    type Primitives = N;
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error> {
        // set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            self.chain_spec.is_spurious_dragon_active_at_block(self.input.number);
        self.evm.db_mut().set_state_clear_flag(state_clear_flag);

        // load the l1 gas oracle contract in cache
        let _ = self
            .evm
            .db_mut()
            .load_cache_account(L1_GAS_PRICE_ORACLE_ADDRESS)
            .map_err(BlockExecutionError::other)?;

        if self
            .chain_spec
            .scroll_fork_activation(ScrollHardfork::Curie)
            .transitions_at_block(self.input.number)
        {
            if let Err(err) = apply_curie_hard_fork(self.evm.db_mut()) {
                tracing::debug!(%err, "failed to apply curie hardfork");
                return Err(
                    ScrollBlockExecutionError::fork(ForkError::Curie(err.to_string())).into()
                );
            };
        }

        Ok(())
    }

    fn execute_transaction(&mut self, tx: Recovered<&N::SignedTx>) -> Result<u64, Self::Error> {
        let chain_spec = &self.chain_spec;
        // The sum of the transaction’s gas limit and the gas utilized in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = self.input.gas_limit - self.gas_used;
        if tx.gas_limit() > block_available_gas {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx.gas_limit(),
                block_available_gas,
            }
            .into())
        }

        // verify the transaction type is accepted by the current fork.
        if tx.is_eip2930() && !chain_spec.is_curie_active_at_block(self.input.number) {
            return Err(ScrollBlockExecutionError::consensus(ConsensusError::InvalidTransaction(
                InvalidTransactionError::Eip2930Disabled,
            ))
            .into())
        }
        if tx.is_eip1559() && !chain_spec.is_curie_active_at_block(self.input.number) {
            return Err(ScrollBlockExecutionError::consensus(ConsensusError::InvalidTransaction(
                InvalidTransactionError::Eip1559Disabled,
            ))
            .into())
        }
        if tx.is_eip4844() {
            return Err(ScrollBlockExecutionError::consensus(ConsensusError::InvalidTransaction(
                InvalidTransactionError::Eip4844Disabled,
            ))
            .into())
        }
        if tx.is_eip7702() {
            return Err(ScrollBlockExecutionError::consensus(ConsensusError::InvalidTransaction(
                InvalidTransactionError::Eip7702Disabled,
            ))
            .into())
        }

        let hash = tx.tx_hash();

        // disable the base fee checks for l1 messages.
        self.evm.with_base_fee_check(!tx.is_l1_message());

        // execute the transaction and commit the result to the database
        let ResultAndState { result, state } =
            self.evm.transact(&tx).map_err(move |err| BlockExecutionError::evm(err, *hash))?;
        self.evm.db_mut().commit(state);

        trace!(target: "evm", ?tx, "executed transaction");

        let l1_fee = if tx.is_l1_message() {
            U256::ZERO
        } else {
            // compute l1 fee for all non-l1 transaction
            self.evm.l1_fee().expect("l1 fee loaded")
        };

        let gas_used = result.gas_used();
        self.gas_used += gas_used;

        let ctx = ReceiptBuilderCtx::<'_, N::SignedTx, E::HaltReason> {
            tx: tx.tx(),
            result,
            cumulative_gas_used: self.gas_used,
            l1_fee,
        };
        self.receipts.push(self.receipt_builder.build_receipt(ctx));

        Ok(gas_used)
    }

    fn apply_post_execution_changes(self) -> Result<BlockExecutionResult<N::Receipt>, Self::Error> {
        Ok(BlockExecutionResult {
            receipts: self.receipts,
            requests: Default::default(),
            gas_used: self.gas_used,
        })
    }

    fn with_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}
}

/// Helper type with backwards compatible methods to obtain Scroll executor
/// providers.
#[derive(Debug)]
pub struct ScrollExecutorProvider;

impl ScrollExecutorProvider {
    /// Creates a new default scroll executor provider.
    pub fn scroll(
        chain_spec: Arc<ScrollChainSpec>,
    ) -> BasicBlockExecutorProvider<ScrollExecutionStrategyFactory> {
        let evm_config = ScrollEvmConfig::new(chain_spec);
        let receipt_builder = BasicScrollReceiptBuilder::default();
        BasicBlockExecutorProvider::new(ScrollExecutionStrategyFactory::new(
            evm_config,
            receipt_builder,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScrollExecutionStrategy;
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_evm::execute::ProviderError;
    use reth_primitives::{Block, BlockBody};
    use reth_primitives_traits::transaction::signed::{
        SignedTransaction, SignedTransactionIntoRecoveredExt,
    };
    use reth_revm::db::{states::bundle_state::BundleRetention, EmptyDBTyped};
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpecBuilder};
    use reth_scroll_consensus::{
        BLOB_SCALAR_SLOT, COMMIT_SCALAR_SLOT, CURIE_L1_GAS_PRICE_ORACLE_BYTECODE,
        CURIE_L1_GAS_PRICE_ORACLE_STORAGE, IS_CURIE_SLOT, L1_BASE_FEE_SLOT, L1_BLOB_BASE_FEE_SLOT,
        L1_GAS_PRICE_ORACLE_ADDRESS, OVER_HEAD_SLOT, SCALAR_SLOT,
    };
    use reth_scroll_primitives::{ScrollBlock, ScrollTransactionSigned};
    use revm::{
        bytecode::Bytecode,
        database::states::StorageSlot,
        inspector::NoOpInspector,
        primitives::{Address, TxKind, B256, U256},
        state::AccountInfo,
    };
    use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxType, ScrollTypedTransaction};
    use scroll_alloy_evm::ScrollEvm;

    const BLOCK_GAS_LIMIT: u64 = 10_000_000;
    const SCROLL_CHAIN_ID: u64 = 534352;
    const NOT_CURIE_BLOCK_NUMBER: u64 = 7096835;
    const CURIE_BLOCK_NUMBER: u64 = 7096837;

    fn state() -> State<EmptyDBTyped<ProviderError>> {
        let db = EmptyDBTyped::<ProviderError>::new();
        State::builder().with_database(db).with_bundle_update().without_state_clear().build()
    }

    fn strategy<'a, 'b>(
        block: &RecoveredBlock<ScrollBlock>,
        state: &'a mut State<EmptyDBTyped<ProviderError>>,
        receipt_builder: &'b BasicScrollReceiptBuilder,
    ) -> ScrollExecutionStrategy<
        'b,
        ScrollEvm<&'a mut State<EmptyDBTyped<ProviderError>>, NoOpInspector>,
        ScrollPrimitives,
        ScrollChainSpec,
    > {
        let chain_spec =
            Arc::new(ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()));
        let evm_config = ScrollEvmConfig::new(chain_spec);

        let evm = evm_config.scroll_evm_for_block(state, block.header());
        ScrollExecutionStrategy::new(
            evm,
            block.sealed_block(),
            evm_config.chain_spec().clone(),
            receipt_builder,
        )
    }

    fn block(
        number: u64,
        transactions: Vec<ScrollTransactionSigned>,
    ) -> RecoveredBlock<<ScrollPrimitives as NodePrimitives>::Block> {
        let senders = transactions.iter().map(|t| t.recover_signer().unwrap()).collect();
        RecoveredBlock::new_unhashed(
            Block {
                header: Header { number, gas_limit: BLOCK_GAS_LIMIT, ..Default::default() },
                body: BlockBody { transactions, ..Default::default() },
            },
            senders,
        )
    }

    fn transaction(typ: ScrollTxType, gas_limit: u64) -> ScrollTransactionSigned {
        let transaction = match typ {
            ScrollTxType::Legacy => ScrollTypedTransaction::Legacy(alloy_consensus::TxLegacy {
                to: TxKind::Call(Address::ZERO),
                chain_id: Some(SCROLL_CHAIN_ID),
                gas_limit,
                ..Default::default()
            }),
            ScrollTxType::Eip2930 => ScrollTypedTransaction::Eip2930(alloy_consensus::TxEip2930 {
                to: TxKind::Call(Address::ZERO),
                chain_id: SCROLL_CHAIN_ID,
                gas_limit,
                ..Default::default()
            }),
            ScrollTxType::Eip1559 => ScrollTypedTransaction::Eip1559(alloy_consensus::TxEip1559 {
                to: TxKind::Call(Address::ZERO),
                chain_id: SCROLL_CHAIN_ID,
                gas_limit,
                ..Default::default()
            }),
            ScrollTxType::L1Message => {
                ScrollTypedTransaction::L1Message(scroll_alloy_consensus::TxL1Message {
                    sender: Address::random(),
                    to: Address::ZERO,
                    gas_limit,
                    ..Default::default()
                })
            }
        };

        let pk = B256::random();
        let signature = reth_primitives::sign_message(pk, transaction.signature_hash()).unwrap();
        ScrollTransactionSigned::new_unhashed(transaction, signature)
    }

    fn execute_transaction(
        tx_type: ScrollTxType,
        block_number: u64,
        expected_l1_fee: U256,
        expected_error: Option<&str>,
    ) -> eyre::Result<()> {
        // prepare transaction
        let transaction = transaction(tx_type, MIN_TRANSACTION_GAS);
        let block = block(block_number, vec![transaction.clone()]);

        // init strategy
        let mut state = state();
        let receipt_builder = BasicScrollReceiptBuilder::default();
        let mut strategy = strategy(&block, &mut state, &receipt_builder);

        // determine l1 gas oracle storage
        let l1_gas_oracle_storage = if strategy.chain_spec.is_curie_active_at_block(block_number) {
            vec![
                (L1_BASE_FEE_SLOT, U256::from(1000)),
                (OVER_HEAD_SLOT, U256::from(1000)),
                (SCALAR_SLOT, U256::from(1000)),
                (L1_BLOB_BASE_FEE_SLOT, U256::from(10000)),
                (COMMIT_SCALAR_SLOT, U256::from(1000)),
                (BLOB_SCALAR_SLOT, U256::from(10000)),
                (IS_CURIE_SLOT, U256::from(1)),
            ]
        } else {
            vec![
                (L1_BASE_FEE_SLOT, U256::from(1000)),
                (OVER_HEAD_SLOT, U256::from(1000)),
                (SCALAR_SLOT, U256::from(1000)),
            ]
        }
        .into_iter()
        .collect();

        // load accounts in state
        strategy.evm.db_mut().insert_account_with_storage(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            Default::default(),
            l1_gas_oracle_storage,
        );
        for add in block.senders() {
            strategy
                .evm
                .db_mut()
                .insert_account(*add, AccountInfo { balance: U256::MAX, ..Default::default() });
        }

        // execute and verify output
        let res = strategy
            .execute_transaction(transaction.try_into_recovered().unwrap().as_recovered_ref());

        // check for error or execution outcome
        let output = strategy.apply_post_execution_changes()?;
        if let Some(error) = expected_error {
            assert_eq!(res.unwrap_err().to_string(), error);
        } else {
            let BlockExecutionResult { receipts, .. } = output;
            let inner = alloy_consensus::Receipt {
                cumulative_gas_used: MIN_TRANSACTION_GAS,
                status: true.into(),
                ..Default::default()
            };
            let into_scroll_receipt = |inner: alloy_consensus::Receipt| {
                ScrollTransactionReceipt::new(inner, expected_l1_fee)
            };
            let receipt = match tx_type {
                ScrollTxType::Legacy => ScrollReceipt::Legacy(into_scroll_receipt(inner)),
                ScrollTxType::Eip2930 => ScrollReceipt::Eip2930(into_scroll_receipt(inner)),
                ScrollTxType::Eip1559 => ScrollReceipt::Eip1559(into_scroll_receipt(inner)),
                ScrollTxType::L1Message => ScrollReceipt::L1Message(inner),
            };
            let expected = vec![receipt];

            assert_eq!(receipts, expected);
        }

        Ok(())
    }

    #[test]
    fn test_apply_pre_execution_changes_curie_block() -> eyre::Result<()> {
        // init curie transition block
        let curie_block = block(7096836, vec![]);

        // init strategy
        let mut state = state();
        let receipt_builder = BasicScrollReceiptBuilder::default();
        let mut strategy = strategy(&curie_block, &mut state, &receipt_builder);

        // apply pre execution change
        strategy.apply_pre_execution_changes()?;

        // take bundle
        let state = strategy.evm.db_mut();
        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();

        // assert oracle contract contains updated bytecode
        let oracle = bundle.state.get(&L1_GAS_PRICE_ORACLE_ADDRESS).unwrap().clone();
        let bytecode = Bytecode::new_raw(CURIE_L1_GAS_PRICE_ORACLE_BYTECODE);
        assert_eq!(oracle.info.unwrap().code.unwrap(), bytecode);

        // check oracle contract contains storage changeset
        let mut storage = oracle.storage.into_iter().collect::<Vec<(U256, StorageSlot)>>();
        storage.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (got, expected) in storage.into_iter().zip(CURIE_L1_GAS_PRICE_ORACLE_STORAGE) {
            assert_eq!(got.0, expected.0);
            assert_eq!(got.1, StorageSlot { present_value: expected.1, ..Default::default() });
        }

        Ok(())
    }

    #[test]
    fn test_apply_pre_execution_changes_not_curie_block() -> eyre::Result<()> {
        // init block
        let not_curie_block = block(7096837, vec![]);

        // init strategy
        let mut state = state();
        let receipt_builder = BasicScrollReceiptBuilder::default();
        let mut strategy = strategy(&not_curie_block, &mut state, &receipt_builder);

        // apply pre execution change
        strategy.apply_pre_execution_changes()?;

        // take bundle
        let state = strategy.evm.db_mut();
        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();

        // assert oracle contract is empty
        let oracle = bundle.state.get(&L1_GAS_PRICE_ORACLE_ADDRESS);
        assert!(oracle.is_none());

        Ok(())
    }

    #[test]
    fn test_execute_transactions_exceeds_block_gas_limit() -> eyre::Result<()> {
        // prepare transaction exceeding block gas limit
        let transaction = transaction(ScrollTxType::Legacy, BLOCK_GAS_LIMIT + 1);
        let block = block(7096837, vec![transaction.clone()]);

        // init strategy
        let mut state = state();
        let receipt_builder = BasicScrollReceiptBuilder::default();
        let mut strategy = strategy(&block, &mut state, &receipt_builder);

        // execute and verify error
        let res = strategy.execute_transaction(
            transaction.try_into_recovered().expect("failed to recover tx").as_recovered_ref(),
        );
        assert_eq!(
            res.unwrap_err().to_string(),
            "transaction gas limit 10000001 is more than blocks available gas 10000000"
        );

        Ok(())
    }

    #[test]
    fn test_execute_transactions_l1_message() -> eyre::Result<()> {
        // Execute l1 message on curie block
        let expected_l1_fee = U256::ZERO;
        execute_transaction(ScrollTxType::L1Message, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_curie_fork() -> eyre::Result<()> {
        // Execute legacy transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transaction(ScrollTxType::Legacy, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_not_curie_fork() -> eyre::Result<()> {
        // Execute legacy before curie block
        let expected_l1_fee = U256::from(2);
        execute_transaction(ScrollTxType::Legacy, NOT_CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transaction(ScrollTxType::Eip2930, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_not_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction before curie block
        execute_transaction(
            ScrollTxType::Eip2930,
            NOT_CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("failed to validate block: EIP-2930 transactions are disabled"),
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip1559_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transaction(ScrollTxType::Eip1559, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip_not_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction before curie block
        execute_transaction(
            ScrollTxType::Eip1559,
            NOT_CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("failed to validate block: EIP-1559 transactions are disabled"),
        )?;
        Ok(())
    }
}
