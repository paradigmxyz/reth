//! Execution primitives for EVM.

use crate::ScrollEvmConfig;
use core::fmt::Debug;

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, B256};
use reth_primitives::SealedBlock;
use reth_primitives_traits::Block;

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

/// Helper type with backwards compatible methods to obtain Scroll executor
/// providers.
pub type ScrollExecutorProvider = ScrollEvmConfig;

#[cfg(test)]
mod tests {
    use crate::{ScrollEvmConfig, ScrollRethReceiptBuilder};
    use std::{convert::Infallible, sync::Arc};

    use alloy_consensus::{
        transaction::{Recovered, SignerRecoverable},
        Block, BlockBody, Header, SignableTransaction, Signed, TxLegacy,
    };
    use alloy_eips::{
        eip7702::{constants::PER_EMPTY_ACCOUNT_COST, Authorization, SignedAuthorization},
        Typed2718,
    };
    use alloy_evm::{
        block::{BlockExecutionResult, BlockExecutor},
        precompiles::PrecompilesMap,
        Evm,
    };
    use alloy_primitives::Sealed;
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_evm::ConfigureEvm;
    use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SignedTransaction};
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpec, ScrollChainSpecBuilder};
    use reth_scroll_primitives::{
        ScrollBlock, ScrollPrimitives, ScrollReceipt, ScrollTransactionSigned,
    };
    use revm::{
        bytecode::Bytecode,
        database::{
            states::{bundle_state::BundleRetention, StorageSlot},
            EmptyDBTyped, State,
        },
        inspector::NoOpInspector,
        primitives::{Address, TxKind, B256, U256},
        state::AccountInfo,
    };
    use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxEnvelope, ScrollTxType};
    use scroll_alloy_evm::{
        curie::{
            BLOB_SCALAR_SLOT, COMMIT_SCALAR_SLOT, CURIE_L1_GAS_PRICE_ORACLE_BYTECODE,
            CURIE_L1_GAS_PRICE_ORACLE_STORAGE, IS_CURIE_SLOT, L1_BLOB_BASE_FEE_SLOT,
            L1_GAS_PRICE_ORACLE_ADDRESS,
        },
        ScrollBlockExecutor, ScrollEvm,
    };
    use scroll_alloy_hardforks::ScrollHardforks;

    const BLOCK_GAS_LIMIT: u64 = 10_000_000;
    const SCROLL_CHAIN_ID: u64 = 534352;
    const NOT_CURIE_BLOCK_NUMBER: u64 = 7096835;
    const CURIE_BLOCK_NUMBER: u64 = 7096837;
    const EUCLID_V2_BLOCK_NUMBER: u64 = 14907015;
    const EUCLID_V2_BLOCK_TIMESTAMP: u64 = 1745305200;

    const L1_BASE_FEE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);
    const OVER_HEAD_SLOT: U256 = U256::from_limbs([2, 0, 0, 0]);
    const SCALAR_SLOT: U256 = U256::from_limbs([3, 0, 0, 0]);

    fn state() -> State<EmptyDBTyped<Infallible>> {
        let db = EmptyDBTyped::<Infallible>::new();
        State::builder().with_database(db).with_bundle_update().without_state_clear().build()
    }

    #[allow(clippy::type_complexity)]
    fn executor<'a>(
        block: &RecoveredBlock<ScrollBlock>,
        state: &'a mut State<EmptyDBTyped<Infallible>>,
    ) -> ScrollBlockExecutor<
        ScrollEvm<&'a mut State<EmptyDBTyped<Infallible>>, NoOpInspector, PrecompilesMap>,
        ScrollRethReceiptBuilder,
        Arc<ScrollChainSpec>,
    > {
        let chain_spec =
            Arc::new(ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()));
        let evm_config = ScrollEvmConfig::scroll(chain_spec.clone());

        let evm = evm_config.evm_for_block(state, block.header());
        let receipt_builder = ScrollRethReceiptBuilder::default();
        ScrollBlockExecutor::new(evm, chain_spec, receipt_builder)
    }

    fn block(
        number: u64,
        timestamp: u64,
        transactions: Vec<ScrollTransactionSigned>,
    ) -> RecoveredBlock<<ScrollPrimitives as NodePrimitives>::Block> {
        let senders = transactions.iter().map(|t| t.recover_signer().unwrap()).collect();
        RecoveredBlock::new_unhashed(
            Block {
                header: Header {
                    number,
                    timestamp,
                    gas_limit: BLOCK_GAS_LIMIT,
                    ..Default::default()
                },
                body: BlockBody { transactions, ..Default::default() },
            },
            senders,
        )
    }

    fn transaction(typ: ScrollTxType, gas_limit: u64) -> ScrollTxEnvelope {
        let pk = B256::random();
        match typ {
            ScrollTxType::Legacy => {
                let tx = TxLegacy {
                    to: TxKind::Call(Address::ZERO),
                    chain_id: Some(SCROLL_CHAIN_ID),
                    gas_limit,
                    ..Default::default()
                };
                let signature = reth_primitives::sign_message(pk, tx.signature_hash()).unwrap();
                ScrollTxEnvelope::Legacy(Signed::new_unhashed(tx, signature))
            }
            ScrollTxType::Eip2930 => {
                let tx = alloy_consensus::TxEip2930 {
                    to: TxKind::Call(Address::ZERO),
                    chain_id: SCROLL_CHAIN_ID,
                    gas_limit,
                    ..Default::default()
                };
                let signature = reth_primitives::sign_message(pk, tx.signature_hash()).unwrap();
                ScrollTxEnvelope::Eip2930(Signed::new_unhashed(tx, signature))
            }
            ScrollTxType::Eip1559 => {
                let tx = alloy_consensus::TxEip1559 {
                    to: TxKind::Call(Address::ZERO),
                    chain_id: SCROLL_CHAIN_ID,
                    gas_limit,
                    ..Default::default()
                };
                let signature = reth_primitives::sign_message(pk, tx.signature_hash()).unwrap();
                ScrollTxEnvelope::Eip1559(Signed::new_unhashed(tx, signature))
            }
            ScrollTxType::Eip7702 => {
                let authorization = Authorization {
                    chain_id: Default::default(),
                    address: Address::random(),
                    nonce: 0,
                };
                let signature =
                    reth_primitives::sign_message(B256::random(), authorization.signature_hash())
                        .unwrap();

                let tx = alloy_consensus::TxEip7702 {
                    to: Address::ZERO,
                    chain_id: SCROLL_CHAIN_ID,
                    gas_limit: gas_limit + PER_EMPTY_ACCOUNT_COST,
                    authorization_list: vec![SignedAuthorization::new_unchecked(
                        authorization,
                        signature.v() as u8,
                        signature.r(),
                        signature.s(),
                    )],
                    ..Default::default()
                };
                let signature = reth_primitives::sign_message(pk, tx.signature_hash()).unwrap();
                ScrollTxEnvelope::Eip7702(Signed::new_unhashed(tx, signature))
            }
            ScrollTxType::L1Message => {
                ScrollTxEnvelope::L1Message(Sealed::new(scroll_alloy_consensus::TxL1Message {
                    sender: Address::random(),
                    to: Address::ZERO,
                    gas_limit,
                    ..Default::default()
                }))
            }
        }
    }

    fn execute_transaction(
        tx_type: ScrollTxType,
        block_number: u64,
        block_timestamp: u64,
        expected_l1_fee: U256,
        expected_error: Option<&str>,
    ) -> eyre::Result<()> {
        // prepare transaction
        let transaction = transaction(tx_type, MIN_TRANSACTION_GAS);
        let block = block(block_number, block_timestamp, vec![transaction.clone()]);

        // init strategy
        let mut state = state();
        let mut strategy = executor(&block, &mut state);

        // determine l1 gas oracle storage
        let l1_gas_oracle_storage = if strategy.spec().is_curie_active_at_block(block_number) {
            vec![
                (L1_BLOB_BASE_FEE_SLOT, U256::from(1000)),
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
        strategy.evm_mut().db_mut().insert_account_with_storage(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            Default::default(),
            l1_gas_oracle_storage,
        );
        for add in block.senders() {
            strategy
                .evm_mut()
                .db_mut()
                .insert_account(*add, AccountInfo { balance: U256::MAX, ..Default::default() });
        }

        // execute and verify output
        let sender = transaction.try_recover()?;
        let tx = Recovered::new_unchecked(transaction, sender);
        let res = strategy.execute_transaction(&tx);

        // check for error or execution outcome
        let output = strategy.apply_post_execution_changes()?;
        if let Some(error) = expected_error {
            assert!(res.unwrap_err().to_string().contains(error));
        } else {
            let BlockExecutionResult { receipts, .. } = output;
            let gas_used =
                MIN_TRANSACTION_GAS + if tx_type.is_eip7702() { PER_EMPTY_ACCOUNT_COST } else { 0 };
            let inner = alloy_consensus::Receipt {
                cumulative_gas_used: gas_used,
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
                ScrollTxType::Eip7702 => ScrollReceipt::Eip7702(into_scroll_receipt(inner)),
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
        let curie_block = block(CURIE_BLOCK_NUMBER - 1, 0, vec![]);

        // init strategy
        let mut state = state();
        let mut strategy = executor(&curie_block, &mut state);

        // apply pre execution change
        strategy.apply_pre_execution_changes()?;

        // take bundle
        let state = strategy.evm_mut().db_mut();
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
        let not_curie_block = block(NOT_CURIE_BLOCK_NUMBER, 0, vec![]);

        // init strategy
        let mut state = state();
        let mut strategy = executor(&not_curie_block, &mut state);

        // apply pre execution change
        strategy.apply_pre_execution_changes()?;

        // take bundle
        let state = strategy.evm_mut().db_mut();
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
        let block = block(7096837, 0, vec![transaction.clone()]);

        // init strategy
        let mut state = state();
        let mut strategy = executor(&block, &mut state);

        // execute and verify error
        let sender = transaction.try_recover()?;
        let tx = Recovered::new_unchecked(transaction, sender);
        let res = strategy.execute_transaction(&tx);
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
        execute_transaction(ScrollTxType::L1Message, CURIE_BLOCK_NUMBER, 0, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_curie_fork() -> eyre::Result<()> {
        // Execute legacy transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transaction(ScrollTxType::Legacy, CURIE_BLOCK_NUMBER, 0, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_legacy_not_curie_fork() -> eyre::Result<()> {
        // Execute legacy before curie block
        let expected_l1_fee = U256::from(2);
        execute_transaction(
            ScrollTxType::Legacy,
            NOT_CURIE_BLOCK_NUMBER,
            0,
            expected_l1_fee,
            None,
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transaction(ScrollTxType::Eip2930, CURIE_BLOCK_NUMBER, 0, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip2930_not_curie_fork() -> eyre::Result<()> {
        // Execute eip2930 transaction before curie block
        execute_transaction(
            ScrollTxType::Eip2930,
            NOT_CURIE_BLOCK_NUMBER,
            0,
            U256::ZERO,
            Some("Eip2930 is not supported"),
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip1559_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction on curie block
        let expected_l1_fee = U256::from(10);
        execute_transaction(ScrollTxType::Eip1559, CURIE_BLOCK_NUMBER, 0, expected_l1_fee, None)?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip1559_not_curie_fork() -> eyre::Result<()> {
        // Execute eip1559 transaction before curie block
        execute_transaction(
            ScrollTxType::Eip1559,
            NOT_CURIE_BLOCK_NUMBER,
            0,
            U256::ZERO,
            Some("Eip1559 is not supported"),
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip7702_euclid_v2_fork() -> eyre::Result<()> {
        // Execute eip7702 transaction on euclid v2 block.
        let expected_l1_fee = U256::from(19);
        execute_transaction(
            ScrollTxType::Eip7702,
            EUCLID_V2_BLOCK_NUMBER,
            EUCLID_V2_BLOCK_TIMESTAMP,
            expected_l1_fee,
            None,
        )?;
        Ok(())
    }

    #[test]
    fn test_execute_transactions_eip7702_not_euclid_v2_fork() -> eyre::Result<()> {
        // Execute eip7702 transaction before euclid v2 block
        execute_transaction(
            ScrollTxType::Eip7702,
            EUCLID_V2_BLOCK_NUMBER - 1,
            EUCLID_V2_BLOCK_TIMESTAMP - 1,
            U256::ZERO,
            Some("Eip7702 is not supported"),
        )?;
        Ok(())
    }
}
