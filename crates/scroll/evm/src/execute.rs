//! Implementation of the [`BlockExecutionStrategy`] for Scroll.

use crate::{
    config::{ScrollConfigureEvm, ScrollEvmT},
    receipt::{BasicScrollReceiptBuilder, ReceiptBuilderCtx, ScrollReceiptBuilder},
    ForkError, ScrollEvmConfig,
};
use alloy_consensus::{Header, Transaction, TxReceipt, Typed2718};
use alloy_eips::eip7685::Requests;
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_evm::{
    execute::{
        BasicBlockExecutorProvider, BlockExecutionError, BlockExecutionStrategy,
        BlockExecutionStrategyFactory, BlockValidationError, ExecuteOutput,
        InternalBlockExecutionError,
    },
    Database, Evm,
};
use reth_primitives::{gas_spent_by_transactions, GotExpected, InvalidTransactionError};
use reth_primitives_traits::{BlockBody, NodePrimitives, RecoveredBlock, SignedTransaction};
use reth_revm::primitives::U256;
use reth_scroll_chainspec::{ChainSpecProvider, ScrollChainSpec};
use reth_scroll_consensus::{apply_curie_hard_fork, L1_GAS_PRICE_ORACLE_ADDRESS};
use reth_scroll_forks::{ScrollHardfork, ScrollHardforks};
use reth_scroll_primitives::{transaction::signed::IsL1Message, ScrollPrimitives, ScrollReceipt};
use revm::{
    primitives::{ExecutionResult, ResultAndState},
    DatabaseCommit, State,
};
use std::{fmt::Debug, sync::Arc};
use tracing::trace;

/// The Scroll block execution strategy.
#[derive(Debug)]
pub struct ScrollExecutionStrategy<DB, N: NodePrimitives, EvmConfig> {
    /// Evm configuration.
    evm_config: EvmConfig,
    /// Current state for the execution.
    state: State<DB>,
    /// Receipt builder.
    receipt_builder: Arc<dyn ScrollReceiptBuilder<N::SignedTx, Receipt = N::Receipt>>,
}

impl<DB, N, EvmConfig> ScrollExecutionStrategy<DB, N, EvmConfig>
where
    N: NodePrimitives,
{
    /// Returns an instance of [`ScrollExecutionStrategy`].
    pub const fn new(
        evm_config: EvmConfig,
        state: State<DB>,
        receipt_builder: Arc<dyn ScrollReceiptBuilder<N::SignedTx, Receipt = N::Receipt>>,
    ) -> Self {
        Self { evm_config, state, receipt_builder }
    }
}

impl<DB, N, EvmConfig> BlockExecutionStrategy for ScrollExecutionStrategy<DB, N, EvmConfig>
where
    DB: Database,
    N: NodePrimitives<BlockHeader = Header, Receipt = ScrollReceipt, SignedTx: IsL1Message>,
    EvmConfig: ScrollConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + ChainSpecProvider<ChainSpec = ScrollChainSpec>,
{
    type DB = DB;
    type Primitives = N;
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(
        &mut self,
        block: &RecoveredBlock<N::Block>,
    ) -> Result<(), Self::Error> {
        // set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag = (*self.evm_config.chain_spec())
            .is_spurious_dragon_active_at_block(block.header().number);
        self.state.set_state_clear_flag(state_clear_flag);

        // load the l1 gas oracle contract in cache
        let _ = self.state.load_cache_account(L1_GAS_PRICE_ORACLE_ADDRESS).map_err(|err| {
            BlockExecutionError::Internal(InternalBlockExecutionError::other(err))
        })?;

        if self
            .evm_config
            .chain_spec()
            .fork(ScrollHardfork::Curie)
            .transitions_at_block(block.number)
        {
            if let Err(err) = apply_curie_hard_fork(&mut self.state) {
                tracing::debug!(%err, "failed to apply curie hardfork");
                return Err(ForkError::Curie(err.to_string()).into());
            };
        }

        Ok(())
    }

    fn execute_transactions(
        &mut self,
        block: &RecoveredBlock<N::Block>,
    ) -> Result<ExecuteOutput<N::Receipt>, Self::Error> {
        let mut evm = self.evm_config.scroll_evm_for_block(&mut self.state, block.header());

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body().transactions().len());
        let chain_spec = self.evm_config.chain_spec();

        for (sender, transaction) in block.transactions_with_sender() {
            // The sum of the transaction’s gas limit and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header().gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            // verify the transaction type is accepted by the current fork.
            if transaction.is_eip2930() && !chain_spec.is_curie_active_at_block(block.number) {
                return Err(ConsensusError::InvalidTransaction(
                    InvalidTransactionError::Eip2930Disabled,
                )
                .into())
            }
            if transaction.is_eip1559() && !chain_spec.is_curie_active_at_block(block.number) {
                return Err(ConsensusError::InvalidTransaction(
                    InvalidTransactionError::Eip1559Disabled,
                )
                .into())
            }
            if transaction.is_eip4844() {
                return Err(ConsensusError::InvalidTransaction(
                    InvalidTransactionError::Eip4844Disabled,
                )
                .into())
            }
            if transaction.is_eip7702() {
                return Err(ConsensusError::InvalidTransaction(
                    InvalidTransactionError::Eip7702Disabled,
                )
                .into())
            }

            let tx_env = self.evm_config.tx_env(transaction, *sender);

            // disable the base fee checks for l1 messages.
            evm.with_base_fee_check(!transaction.is_l1_message());

            // execute the transaction and commit the result to the database
            let ResultAndState { result, state } =
                evm.transact(tx_env).map_err(|err| BlockValidationError::EVM {
                    hash: transaction.recalculate_hash(),
                    error: Box::new(err),
                })?;
            evm.db_mut().commit(state);

            trace!(target: "evm", ?transaction, "executed transaction");

            let l1_fee = if transaction.is_l1_message() {
                // l1 messages do not get any gas refunded
                if let ExecutionResult::Success { gas_refunded, .. } = result {
                    cumulative_gas_used += gas_refunded
                }

                U256::ZERO
            } else {
                // compute l1 fee for all non-l1 transaction
                evm.l1_fee().expect("l1 fee loaded")
            };

            cumulative_gas_used += result.gas_used();

            let ctx = ReceiptBuilderCtx {
                header: block.header(),
                tx: transaction,
                result,
                cumulative_gas_used,
                l1_fee,
            };
            receipts.push(self.receipt_builder.build_receipt(ctx))
        }

        Ok(ExecuteOutput { receipts, gas_used: cumulative_gas_used })
    }

    fn apply_post_execution_changes(
        &mut self,
        _block: &RecoveredBlock<N::Block>,
        _receipts: &[N::Receipt],
    ) -> Result<Requests, Self::Error> {
        Ok(Default::default())
    }

    fn state_ref(&self) -> &State<DB> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut State<DB> {
        &mut self.state
    }

    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<N::Block>,
        receipts: &[N::Receipt],
        _requests: &Requests,
    ) -> Result<(), ConsensusError> {
        // verify the block gas used
        let cumulative_gas_used = receipts.last().map(|r| r.cumulative_gas_used()).unwrap_or(0);
        if block.gas_used != cumulative_gas_used {
            return Err(ConsensusError::BlockGasUsed {
                gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
                gas_spent_by_tx: gas_spent_by_transactions(receipts),
            });
        }

        // verify the receipts logs bloom and root
        if self.evm_config.chain_spec().is_byzantium_active_at_block(block.header().number) {
            if let Err(error) = reth_ethereum_consensus::verify_receipts(
                block.header().receipts_root,
                block.header().logs_bloom,
                receipts,
            ) {
                tracing::debug!(
                    %error,
                    ?receipts,
                    header_receipt_root = ?block.header().receipts_root,
                    header_bloom = ?block.header().logs_bloom,
                    "failed to verify receipts"
                );
                return Err(error);
            }
        }

        Ok(())
    }
}

/// The factory for a [`ScrollExecutionStrategy`].
#[derive(Clone, Debug)]
pub struct ScrollExecutionStrategyFactory<
    N: NodePrimitives = ScrollPrimitives,
    EvmConfig = ScrollEvmConfig,
> {
    /// The Evm configuration for the [`ScrollExecutionStrategy`].
    evm_config: EvmConfig,
    /// Receipt builder.
    receipt_builder: Arc<dyn ScrollReceiptBuilder<N::SignedTx, Receipt = N::Receipt>>,
}

impl ScrollExecutionStrategyFactory {
    /// Returns a default Scroll factory.
    pub fn scroll(chain_spec: Arc<ScrollChainSpec>) -> Self {
        Self::new(ScrollEvmConfig::new(chain_spec), BasicScrollReceiptBuilder::default())
    }
}

impl<N: NodePrimitives, EvmConfig> ScrollExecutionStrategyFactory<N, EvmConfig>
where
    EvmConfig: Clone,
{
    /// Returns a new instance of the [`ScrollExecutionStrategyFactory`].
    pub fn new(
        evm_config: EvmConfig,
        receipt_builder: impl ScrollReceiptBuilder<N::SignedTx, Receipt = N::Receipt>,
    ) -> Self {
        Self { evm_config, receipt_builder: Arc::new(receipt_builder) }
    }

    /// Returns the EVM configuration for the strategy factory.
    pub fn evm_config(&self) -> EvmConfig {
        self.evm_config.clone()
    }
}

impl<N, EvmConfig> BlockExecutionStrategyFactory for ScrollExecutionStrategyFactory<N, EvmConfig>
where
    N: NodePrimitives<BlockHeader = Header, Receipt = ScrollReceipt, SignedTx: IsL1Message>,
    EvmConfig: ScrollConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + ChainSpecProvider<ChainSpec = ScrollChainSpec>,
{
    type Primitives = N;
    type Strategy<DB: Database> = ScrollExecutionStrategy<DB, N, EvmConfig>;

    /// Creates a strategy using the give database.
    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: Database,
    {
        let state =
            State::builder().with_database(db).without_state_clear().with_bundle_update().build();
        ScrollExecutionStrategy::new(self.evm_config.clone(), state, self.receipt_builder.clone())
    }
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
    use crate::{ScrollEvmConfig, ScrollExecutionStrategy, ScrollExecutionStrategyFactory};
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_evm::execute::{ExecuteOutput, ProviderError};
    use reth_primitives::{Block, BlockBody};
    use reth_primitives_traits::transaction::signed::SignedTransaction;
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpecBuilder};
    use reth_scroll_consensus::{
        BLOB_SCALAR_SLOT, COMMIT_SCALAR_SLOT, CURIE_L1_GAS_PRICE_ORACLE_BYTECODE,
        CURIE_L1_GAS_PRICE_ORACLE_STORAGE, IS_CURIE_SLOT, L1_BASE_FEE_SLOT, L1_BLOB_BASE_FEE_SLOT,
        L1_GAS_PRICE_ORACLE_ADDRESS, OVER_HEAD_SLOT, SCALAR_SLOT,
    };
    use reth_scroll_primitives::ScrollTransactionSigned;
    use revm::{
        db::{
            states::{bundle_state::BundleRetention, StorageSlot},
            EmptyDBTyped,
        },
        primitives::{AccountInfo, Address, Bytecode, TxKind, B256, U256},
    };
    use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxType, ScrollTypedTransaction};

    const BLOCK_GAS_LIMIT: u64 = 10_000_000;
    const SCROLL_CHAIN_ID: u64 = 534352;
    const NOT_CURIE_BLOCK_NUMBER: u64 = 7096835;
    const CURIE_BLOCK_NUMBER: u64 = 7096837;

    fn strategy(
    ) -> ScrollExecutionStrategy<EmptyDBTyped<ProviderError>, ScrollPrimitives, ScrollEvmConfig>
    {
        let chain_spec =
            Arc::new(ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()));
        let evm_config = ScrollEvmConfig::new(chain_spec);
        let factory =
            ScrollExecutionStrategyFactory::new(evm_config, BasicScrollReceiptBuilder::default());
        let db = EmptyDBTyped::<ProviderError>::new();

        factory.create_strategy(db)
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

    fn execute_transactions(
        tx_type: ScrollTxType,
        block_number: u64,
        expected_l1_fee: U256,
        expected_error: Option<&str>,
    ) -> eyre::Result<()> {
        // init strategy
        let mut strategy = strategy();

        // prepare transaction
        let transaction = transaction(tx_type, MIN_TRANSACTION_GAS);
        let block = block(block_number, vec![transaction]);

        // determine l1 gas oracle storage
        let l1_gas_oracle_storage =
            if strategy.evm_config.chain_spec().is_curie_active_at_block(block_number) {
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
        strategy.state.insert_account_with_storage(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            Default::default(),
            l1_gas_oracle_storage,
        );
        for add in block.senders() {
            strategy
                .state
                .insert_account(*add, AccountInfo { balance: U256::MAX, ..Default::default() });
        }

        // execute and verify output
        let res = strategy.execute_transactions(&block);

        // check for error or execution outcome
        if let Some(error) = expected_error {
            assert_eq!(res.unwrap_err().to_string(), error);
        } else {
            let ExecuteOutput { receipts, .. } = res?;
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
        // init strategy
        let mut strategy = strategy();

        // init curie transition block
        let curie_block = block(7096836, vec![]);

        // apply pre execution change
        strategy.apply_pre_execution_changes(&curie_block)?;

        // take bundle
        let mut state = strategy.state;
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
        // init strategy
        let mut strategy = strategy();

        // init block
        let not_curie_block = block(7096837, vec![]);

        // apply pre execution change
        strategy.apply_pre_execution_changes(&not_curie_block)?;

        // take bundle
        let mut state = strategy.state;
        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();

        // assert oracle contract is empty
        let oracle = bundle.state.get(&L1_GAS_PRICE_ORACLE_ADDRESS);
        assert!(oracle.is_none());

        Ok(())
    }

    #[test]
    fn test_execute_transactions_exceeds_block_gas_limit() -> eyre::Result<()> {
        // init strategy
        let mut strategy = strategy();

        // prepare transaction exceeding block gas limit
        let transaction = transaction(ScrollTxType::Legacy, BLOCK_GAS_LIMIT + 1);
        let block = block(7096837, vec![transaction]);

        // execute and verify error
        let res = strategy.execute_transactions(&block);
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
        execute_transactions(ScrollTxType::L1Message, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_curie_fork() -> eyre::Result<()> {
        // Execute legacy transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transactions(ScrollTxType::Legacy, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_not_curie_fork() -> eyre::Result<()> {
        // Execute legacy before curie block
        let expected_l1_fee = U256::from(2);
        execute_transactions(ScrollTxType::Legacy, NOT_CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transactions(ScrollTxType::Eip2930, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_not_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction before curie block
        execute_transactions(
            ScrollTxType::Eip2930,
            NOT_CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("EIP-2930 transactions are disabled"),
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip1559_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transactions(ScrollTxType::Eip1559, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip_not_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction before curie block
        execute_transactions(
            ScrollTxType::Eip1559,
            NOT_CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("EIP-1559 transactions are disabled"),
        )?;
        Ok(())
    }
}
