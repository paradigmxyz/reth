//! Implementation of the [`BlockExecutionStrategy`] for Scroll.

use crate::{ForkError, ScrollEvmConfig};
use alloy_consensus::{Header, Transaction};
use alloy_eips::{eip2718::Encodable2718, eip7685::Requests};
use reth_chainspec::EthereumHardforks;
use reth_consensus::ConsensusError;
use reth_evm::{
    execute::{
        BasicBlockExecutorProvider, BlockExecutionError, BlockExecutionStrategy,
        BlockExecutionStrategyFactory, BlockValidationError, ExecuteOutput, ProviderError,
    },
    ConfigureEvm, ConfigureEvmEnv,
};
use reth_primitives::{
    gas_spent_by_transactions, BlockWithSenders, GotExpected, InvalidTransactionError, Receipt,
    TxType,
};
use reth_revm::primitives::{CfgEnvWithHandlerCfg, U256};
use reth_scroll_chainspec::{ChainSpecProvider, ScrollChainSpec};
use reth_scroll_consensus::apply_curie_hard_fork;
use reth_scroll_execution::FinalizeExecution;
use reth_scroll_forks::{ScrollHardfork, ScrollHardforks};
use revm::{
    db::BundleState,
    primitives::{bytes::BytesMut, BlockEnv, EnvWithHandlerCfg, ResultAndState},
    Database, DatabaseCommit, State,
};
use std::{
    fmt::{Debug, Display},
    sync::Arc,
};

/// The Scroll block execution strategy.
#[derive(Debug)]
pub struct ScrollExecutionStrategy<DB, EvmConfig> {
    /// Evm configuration.
    evm_config: EvmConfig,
    /// Current state for the execution.
    state: State<DB>,
}

impl<DB, EvmConfig> ScrollExecutionStrategy<DB, EvmConfig> {
    /// Returns an instance of [`ScrollExecutionStrategy`].
    pub const fn new(evm_config: EvmConfig, state: State<DB>) -> Self {
        Self { evm_config, state }
    }
}

impl<DB, EvmConfig> ScrollExecutionStrategy<DB, EvmConfig>
where
    EvmConfig: ConfigureEvmEnv<Header = Header>,
{
    /// Configures a new evm configuration and block environment for the given block.
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_env_for_block(&self, header: &Header, total_difficulty: U256) -> EnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.evm_config.fill_cfg_and_block_env(&mut cfg, &mut block_env, header, total_difficulty);

        EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default())
    }
}

impl<DB, EvmConfig> BlockExecutionStrategy<DB> for ScrollExecutionStrategy<DB, EvmConfig>
where
    DB: Database<Error: Into<ProviderError> + Display>,
    State<DB>: FinalizeExecution<Output = BundleState>,
    EvmConfig: ConfigureEvm<Header = Header> + ChainSpecProvider<ChainSpec = ScrollChainSpec>,
{
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        _total_difficulty: U256,
    ) -> Result<(), Self::Error> {
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
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<ExecuteOutput, Self::Error> {
        let env = self.evm_env_for_block(&block.header, total_difficulty);
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.transactions.len());
        let chain_spec = self.evm_config.chain_spec();

        for (sender, transaction) in block.transactions_with_sender() {
            // The sum of the transaction’s gas limit and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            // verify the transaction type is accepted by the current fork.
            match transaction.tx_type() {
                TxType::Eip2930 if !chain_spec.is_curie_active_at_block(block.number) => {
                    return Err(ConsensusError::InvalidTransaction(
                        InvalidTransactionError::Eip2930Disabled,
                    )
                    .into())
                }
                TxType::Eip1559 if !chain_spec.is_curie_active_at_block(block.number) => {
                    return Err(ConsensusError::InvalidTransaction(
                        InvalidTransactionError::Eip1559Disabled,
                    )
                    .into())
                }
                TxType::Eip4844 => {
                    return Err(ConsensusError::InvalidTransaction(
                        InvalidTransactionError::Eip4844Disabled,
                    )
                    .into())
                }
                TxType::Eip7702 => {
                    return Err(ConsensusError::InvalidTransaction(
                        InvalidTransactionError::Eip7702Disabled,
                    )
                    .into())
                }
                _ => (),
            };

            self.evm_config.fill_tx_env(evm.tx_mut(), transaction, *sender);
            if transaction.is_l1_message() {
                evm.context.evm.env.cfg.disable_base_fee = true; // disable base fee for l1 msg
            } else {
                // RLP encode the transaction following eip 2718
                let mut buf = BytesMut::with_capacity(transaction.encode_2718_len());
                transaction.encode_2718(&mut buf);
                let transaction_rlp_bytes = buf.freeze();
                evm.context.evm.env.tx.scroll.rlp_bytes = Some(transaction_rlp_bytes.into());
            }
            evm.context.evm.env.tx.scroll.is_l1_msg = transaction.is_l1_message();

            // execute the transaction and commit the result to the database
            let ResultAndState { result, state } =
                evm.transact().map_err(|err| BlockValidationError::EVM {
                    hash: transaction.recalculate_hash(),
                    error: Box::new(err.map_db_err(|e| e.into())),
                })?;
            evm.db_mut().commit(state);

            let l1_fee = if transaction.is_l1_message() {
                U256::ZERO
            } else {
                // compute l1 fee for all non-l1 transaction
                let l1_block_info =
                    evm.context.evm.inner.l1_block_info.as_ref().expect("l1_block_info loaded");
                let transaction_rlp_bytes =
                    evm.context.evm.env.tx.scroll.rlp_bytes.as_ref().expect("rlp_bytes loaded");
                l1_block_info.calculate_tx_l1_cost(transaction_rlp_bytes, evm.handler.cfg.spec_id)
            };

            cumulative_gas_used += result.gas_used();

            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.into_logs(),
                l1_fee,
            })
        }

        Ok(ExecuteOutput { receipts, gas_used: cumulative_gas_used })
    }

    fn apply_post_execution_changes(
        &mut self,
        _block: &BlockWithSenders,
        _total_difficulty: U256,
        _receipts: &[Receipt],
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
        block: &BlockWithSenders,
        receipts: &[Receipt],
        _requests: &Requests,
    ) -> Result<(), ConsensusError> {
        // verify the block gas used
        let cumulative_gas_used = receipts.last().map(|r| r.cumulative_gas_used).unwrap_or(0);
        if block.gas_used != cumulative_gas_used {
            return Err(ConsensusError::BlockGasUsed {
                gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
                gas_spent_by_tx: gas_spent_by_transactions(receipts),
            });
        }

        // verify the receipts logs bloom and root
        if self.evm_config.chain_spec().is_byzantium_active_at_block(block.header.number) {
            if let Err(error) = reth_ethereum_consensus::verify_receipts(
                block.header.receipts_root,
                block.header.logs_bloom,
                receipts,
            ) {
                tracing::debug!(
                    %error,
                    ?receipts,
                    header_receipt_root = ?block.header.receipts_root,
                    header_bloom = ?block.header.logs_bloom,
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
pub struct ScrollExecutionStrategyFactory<EvmConfig = ScrollEvmConfig> {
    /// The Evm configuration for the [`ScrollExecutionStrategy`].
    evm_config: EvmConfig,
}

impl ScrollExecutionStrategyFactory {
    /// Returns a new instance of the [`ScrollExecutionStrategyFactory`].
    pub const fn new(chain_spec: Arc<ScrollChainSpec>) -> Self {
        let evm_config = ScrollEvmConfig::new(chain_spec);
        Self { evm_config }
    }

    /// Returns the EVM configuration for the strategy factory.
    pub fn evm_config(&self) -> ScrollEvmConfig {
        self.evm_config.clone()
    }
}

impl<EvmConfig> BlockExecutionStrategyFactory for ScrollExecutionStrategyFactory<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header> + ChainSpecProvider<ChainSpec = ScrollChainSpec>,
{
    /// Associated strategy type.
    type Strategy<DB: Database<Error: Into<ProviderError> + Display>>
        = ScrollExecutionStrategy<DB, EvmConfig>
    where
        State<DB>: FinalizeExecution<Output = BundleState>;

    /// Creates a strategy using the give database.
    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
        State<DB>: FinalizeExecution<Output = BundleState>,
    {
        let state =
            State::builder().with_database(db).without_state_clear().with_bundle_update().build();
        ScrollExecutionStrategy::new(self.evm_config.clone(), state)
    }
}

/// Helper type with backwards compatible methods to obtain Scroll executor
/// providers.
#[derive(Debug)]
pub struct ScrollExecutorProvider;

impl ScrollExecutorProvider {
    /// Creates a new default scroll executor provider.
    pub const fn scroll(
        chain_spec: Arc<ScrollChainSpec>,
    ) -> BasicBlockExecutorProvider<ScrollExecutionStrategyFactory> {
        BasicBlockExecutorProvider::new(ScrollExecutionStrategyFactory::new(chain_spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollEvmConfig, ScrollExecutionStrategy, ScrollExecutionStrategyFactory};
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_evm::execute::ExecuteOutput;
    use reth_primitives::{Block, BlockBody, BlockWithSenders, Receipt, TransactionSigned, TxType};
    use reth_primitives_traits::transaction::signed::SignedTransaction;
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpecBuilder};
    use reth_scroll_consensus::{
        BLOB_SCALAR_SLOT, COMMIT_SCALAR_SLOT, CURIE_L1_GAS_PRICE_ORACLE_BYTECODE,
        CURIE_L1_GAS_PRICE_ORACLE_STORAGE, IS_CURIE_SLOT, L1_BASE_FEE_SLOT, L1_BLOB_BASE_FEE_SLOT,
        L1_GAS_PRICE_ORACLE_ADDRESS, OVER_HEAD_SLOT, SCALAR_SLOT,
    };
    use revm::{
        db::states::{bundle_state::BundleRetention, StorageSlot},
        primitives::{Address, B256, U256},
        shared::AccountInfo,
        Bytecode, EmptyDBTyped, TxKind,
    };

    const BLOCK_GAS_LIMIT: u64 = 10_000_000;
    const SCROLL_CHAIN_ID: u64 = 534352;
    const NOT_CURIE_BLOCK_NUMBER: u64 = 7096835;
    const CURIE_BLOCK_NUMBER: u64 = 7096837;

    fn strategy() -> ScrollExecutionStrategy<EmptyDBTyped<ProviderError>, ScrollEvmConfig> {
        let chain_spec =
            Arc::new(ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()));
        let factory = ScrollExecutionStrategyFactory::new(chain_spec);
        let db = EmptyDBTyped::<ProviderError>::new();

        factory.create_strategy(db)
    }

    fn block(number: u64, transactions: Vec<TransactionSigned>) -> BlockWithSenders {
        let senders = transactions.iter().map(|t| t.recover_signer().unwrap()).collect();
        BlockWithSenders {
            block: Block {
                header: Header { number, gas_limit: BLOCK_GAS_LIMIT, ..Default::default() },
                body: BlockBody { transactions, ..Default::default() },
            },
            senders,
        }
    }

    fn transaction(typ: TxType, gas_limit: u64) -> TransactionSigned {
        let mut transaction = match typ {
            TxType::Legacy => reth_primitives::Transaction::Legacy(alloy_consensus::TxLegacy {
                to: TxKind::Call(Address::ZERO),
                ..Default::default()
            }),
            TxType::Eip2930 => reth_primitives::Transaction::Eip2930(alloy_consensus::TxEip2930 {
                to: TxKind::Call(Address::ZERO),
                ..Default::default()
            }),
            TxType::Eip1559 => reth_primitives::Transaction::Eip1559(alloy_consensus::TxEip1559 {
                to: TxKind::Call(Address::ZERO),
                ..Default::default()
            }),
            TxType::Eip4844 => reth_primitives::Transaction::Eip4844(alloy_consensus::TxEip4844 {
                to: Address::ZERO,
                ..Default::default()
            }),
            TxType::Eip7702 => reth_primitives::Transaction::Eip7702(alloy_consensus::TxEip7702 {
                to: Address::ZERO,
                ..Default::default()
            }),
            TxType::L1Message => {
                reth_primitives::Transaction::L1Message(reth_scroll_primitives::TxL1Message {
                    sender: Address::random(),
                    to: Address::ZERO,
                    ..Default::default()
                })
            }
        };
        transaction.set_chain_id(SCROLL_CHAIN_ID);
        transaction.set_gas_limit(gas_limit);

        let pk = B256::random();
        let signature = reth_primitives::sign_message(pk, transaction.signature_hash()).unwrap();
        TransactionSigned::new_unhashed(transaction, signature)
    }

    fn execute_transactions(
        tx_type: TxType,
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
        for add in &block.senders {
            strategy
                .state
                .insert_account(*add, AccountInfo { balance: U256::MAX, ..Default::default() });
        }

        // execute and verify output
        let res = strategy.execute_transactions(&block, U256::ZERO);

        // check for error or execution outcome
        if let Some(error) = expected_error {
            assert_eq!(res.unwrap_err().to_string(), error);
        } else {
            let ExecuteOutput { receipts, .. } = res?;
            let expected = vec![Receipt {
                tx_type,
                cumulative_gas_used: MIN_TRANSACTION_GAS,
                success: true,
                l1_fee: expected_l1_fee,
                ..Default::default()
            }];

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
        strategy.apply_pre_execution_changes(&curie_block, U256::ZERO)?;

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
        strategy.apply_pre_execution_changes(&not_curie_block, U256::ZERO)?;

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
        let transaction = transaction(TxType::Legacy, BLOCK_GAS_LIMIT + 1);
        let block = block(7096837, vec![transaction]);

        // execute and verify error
        let res = strategy.execute_transactions(&block, U256::ZERO);
        assert_eq!(
            res.unwrap_err().to_string(),
            "transaction gas limit 10000001 is more than blocks available gas 10000000"
        );

        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip4844() -> eyre::Result<()> {
        // Execute eip4844 transaction
        execute_transactions(
            TxType::Eip4844,
            CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("EIP-4844 transactions are disabled"),
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip7702() -> eyre::Result<()> {
        // Execute eip7702 transaction
        execute_transactions(
            TxType::Eip7702,
            CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("EIP-7702 transactions are disabled"),
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_l1_message() -> eyre::Result<()> {
        // Execute l1 message on curie block
        let expected_l1_fee = U256::ZERO;
        execute_transactions(TxType::L1Message, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_curie_fork() -> eyre::Result<()> {
        // Execute legacy transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transactions(TxType::Legacy, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_not_curie_fork() -> eyre::Result<()> {
        // Execute legacy before curie block
        let expected_l1_fee = U256::from(2);
        execute_transactions(TxType::Legacy, NOT_CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transactions(TxType::Eip2930, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_not_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction before curie block
        execute_transactions(
            TxType::Eip2930,
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
        execute_transactions(TxType::Eip1559, CURIE_BLOCK_NUMBER, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip_not_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction before curie block
        execute_transactions(
            TxType::Eip1559,
            NOT_CURIE_BLOCK_NUMBER,
            U256::ZERO,
            Some("EIP-1559 transactions are disabled"),
        )?;
        Ok(())
    }
}
