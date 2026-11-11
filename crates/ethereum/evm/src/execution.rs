//! Tests of the Ethereum execution helpers, which evm2 implements.

#![cfg(test)]
#![allow(unused_imports)]

use crate::dao_fork;
use alloc::{boxed::Box, string::ToString, vec::Vec};
use alloy_consensus::{
    constants::ETH_TO_WEI, transaction::Recovered, BlockHeader, Header, TxReceipt,
};
use alloy_eips::{
    eip2718::Typed2718,
    eip4895::Withdrawal,
    eip6110::{DEPOSIT_REQUEST_TYPE, MAINNET_DEPOSIT_CONTRACT_ADDRESS},
    eip7002::WITHDRAWAL_REQUEST_TYPE,
    eip7251::CONSOLIDATION_REQUEST_TYPE,
    eip7685::Requests,
};
use alloy_primitives::{map::AddressMap, Address, Bytes, Log, B256, U256};
use alloy_sol_types::{sol, SolEvent};
use core::any::Any;
use evm2::{
    block::ethereum::execution::*,
    evm::{
        AccountChangeRef, AccountInfo, StateChangeSink, SystemTx, BEACON_ROOTS_ADDRESS,
        BUILDER_DEPOSIT_REQUEST_ADDRESS, BUILDER_EXIT_REQUEST_ADDRESS,
        CONSOLIDATION_REQUEST_ADDRESS, HISTORY_STORAGE_ADDRESS, WITHDRAWAL_REQUEST_ADDRESS,
    },
    interpreter::InstrStop,
    registry::HandlerError,
    Evm, EvmTypes, SpecId, TxResult, TxResultWithState,
};
use reth_ethereum_forks::EthereumHardforks;
use reth_evm::{
    execute::{map_database_error, map_handler_error},
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError, InvalidTxError,
};
use reth_execution_types::{BlockState, StateUpdate};

mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxLegacy, TxType};
    use alloy_genesis::Genesis;
    use alloy_primitives::{address, Signature, TxKind};
    use evm2::{
        bytecode::Bytecode, env::BlockEnv as EvmBlockEnv, evm::InMemoryDB, interpreter::opcode::op,
    };
    use reth_chainspec::{Chain, ChainSpec, MAINNET};
    use reth_ethereum_forks::{EthereumHardfork, ForkCondition};
    use reth_ethereum_primitives::{Block, BlockBody, Receipt, TransactionSigned};
    use reth_evm::{BlockExecutorFactory, ConfigureEvm, Executor};
    use reth_execution_types::BlockExecutionOutput;
    use reth_primitives_traits::RecoveredBlock;
    use reth_trie_common::{HashedPostState, KeccakKeyHasher};
    use std::sync::{mpsc, Arc};

    sol! {
        event DepositEvent(
            bytes pubkey,
            bytes withdrawal_credentials,
            bytes amount,
            bytes signature,
            bytes index
        );
    }

    type BlockEnv = EvmBlockEnv<evm2::BaseEvmTypes>;

    #[test]
    fn detached_commits_preserve_storage_hooks_and_bal() {
        let caller = Address::with_last_byte(1);
        let contract = address!("0000000000000000000000000000000000001000");
        let mut database = InMemoryDB::default();
        database.insert_account_info(
            &caller,
            AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)),
        );
        database.insert_account_info(
            &contract,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(alloy_primitives::bytes!("5f546001015f5500"))),
        );
        let factory = crate::EthBlockExecutorFactory::new(MAINNET.clone());
        let mut expected = None;
        let mut expected_updates = None;
        for mode in 0..3 {
            for stream_state in [false, true] {
                let env = crate::EthEvmEnv::new(
                    SpecId::AMSTERDAM,
                    BlockEnv { gas_limit: U256::from(10_000_000), ..Default::default() },
                    1,
                );
                let mut evm = factory.evm_with_env(evm2::evm::Db::new(database.clone()), env);
                evm.state_mut().enable_bal_builder();
                let mut block = BlockState::new();
                let mut updates = Vec::new();
                for nonce in 0..2 {
                    evm.state_mut().set_bal_index(alloy_eip7928::BlockAccessIndex::new(nonce + 1));
                    let transaction = Recovered::new_unchecked(
                        evm2::ethereum::TxEnvelope::Legacy(TxLegacy {
                            nonce,
                            gas_limit: 1_000_000,
                            gas_price: 1,
                            to: TxKind::Call(contract),
                            ..Default::default()
                        }),
                        caller,
                    );
                    let mut hook = |state| updates.push(state);
                    if mode == 0 {
                        let result = execute_transaction_with_condition(
                            &mut evm,
                            &mut block,
                            stream_state,
                            &mut hook,
                            &transaction,
                            |_, _| reth_evm::CommitChanges::Yes,
                        )
                        .unwrap()
                        .unwrap();
                        assert!(result.status, "{result:?}");
                    } else {
                        let output =
                            execute_transaction_without_commit(&mut evm, &transaction).unwrap();
                        assert!(output.result.status, "{:?}", output.result);
                        if mode == 1 {
                            let _ = commit_detached_transaction(
                                &mut evm,
                                &mut block,
                                stream_state,
                                &mut hook,
                                output,
                            );
                        } else {
                            commit_pending_state(
                                &mut evm,
                                &mut block,
                                stream_state,
                                &mut hook,
                                &output.pending_state,
                            );
                        }
                    }
                }
                let bundle = block.into_bundle();
                assert_eq!(bundle.storage(&contract, U256::ZERO), Some(U256::from(2)));
                assert_eq!(updates.len(), if stream_state { 2 } else { 0 });
                if stream_state {
                    // Account order within an update follows map iteration and is unspecified.
                    for update in &mut updates {
                        update.accounts.sort_unstable_by_key(|account| account.address);
                    }
                    assert_eq!(expected_updates.get_or_insert_with(|| updates.clone()), &updates);
                }
                let output = (bundle, evm.state_mut().take_bal_builder());
                if let Some(expected) = &expected {
                    assert_eq!(&output, expected);
                } else {
                    expected = Some(output);
                }
            }
        }
    }

    fn execute_block(
        spec: SpecId,
        env: BlockEnv,
        database: InMemoryDB,
        transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
        withdrawals: Option<&[Withdrawal]>,
    ) -> Result<BlockExecutionOutput<Receipt>, BlockExecutionError> {
        let builder = ChainSpec::builder().chain(Chain::mainnet()).genesis(Genesis::default());
        let chain = match spec {
            SpecId::LONDON => builder.london_activated(),
            SpecId::SHANGHAI => builder.shanghai_activated(),
            SpecId::PRAGUE => builder.prague_activated(),
            SpecId::AMSTERDAM => builder.amsterdam_activated(),
            _ => panic!("unsupported test fork"),
        }
        .build();
        let (transactions, senders) = transactions.into_iter().map(Recovered::into_parts).unzip();
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: Header {
                    number: 1,
                    beneficiary: env.beneficiary,
                    gas_limit: env.gas_limit.to(),
                    timestamp: env.timestamp.to(),
                    base_fee_per_gas: Some(env.basefee.to()),
                    excess_blob_gas: Some(0),
                    parent_beacon_block_root: Some(B256::ZERO),
                    ..Default::default()
                },
                body: BlockBody {
                    transactions,
                    withdrawals: withdrawals.map(|withdrawals| withdrawals.to_vec().into()),
                    ..Default::default()
                },
            },
            senders,
        );
        let (tx, rx) = mpsc::channel();
        let config = crate::EthEvmConfig::new(Arc::new(chain));
        let without_hook = config.batch_executor(database.clone()).execute(&block)?;
        let output = config
            .batch_executor(database)
            .execute_with_state_hook(&block, move |state| tx.send(state).unwrap())?;
        assert_eq!(without_hook, output);
        let mut streamed = HashedPostState::default();
        for update in rx.try_iter() {
            streamed.extend(reth_execution_types::state_update_to_hashed_post_state(&update));
        }
        let recomputed = HashedPostState::from_bundle_state::<KeccakKeyHasher>(&output.state.state);
        assert_eq!(streamed.into_sorted(), recomputed.into_sorted());
        Ok(output)
    }

    #[test]
    fn charges_london_sstore_set_gas() {
        let caller = address!("0000000000000000000000000000000000000001");
        let contract = address!("0000000000000000000000000000000000001000");
        let mut database = InMemoryDB::default();
        database.insert_account_info(
            &caller,
            AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)),
        );
        database.insert_account_info(
            &contract,
            AccountInfo::default().with_nonce(1).with_code(Bytecode::new_raw(Bytes::from(vec![
                op::PUSH1,
                10,
                op::PUSH1,
                0,
                op::SSTORE,
                op::STOP,
            ]))),
        );
        let transaction = Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    gas_price: 1_000_000_000,
                    gas_limit: 100_000,
                    to: TxKind::Call(contract),
                    value: U256::from(1),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        );

        let output = execute_block(
            SpecId::LONDON,
            BlockEnv {
                gas_limit: U256::from(1_500_000),
                basefee: U256::from(1),
                ..Default::default()
            },
            database,
            [transaction],
            None,
        )
        .expect("EVM execution succeeds");

        assert_eq!(output.result.receipts[0].cumulative_gas_used, 43_106);
        assert!(output.result.receipts[0].success);
        assert_eq!(output.account(&contract).unwrap().unwrap().balance, U256::from(1));
        let sender = output.account(&caller).unwrap().unwrap();
        assert_eq!(sender.nonce, 1);
        assert_eq!(sender.balance, U256::from(ETH_TO_WEI) - U256::from(43_106_000_000_001u64));
        assert_eq!(output.storage(&contract, U256::ZERO).unwrap(), U256::from(10));
    }

    #[test]
    fn applies_withdrawals_to_block_output() {
        let existing = address!("0000000000000000000000000000000000000001");
        let new = address!("0000000000000000000000000000000000000002");
        let mut database = InMemoryDB::default();
        database
            .insert_account_info(&existing, AccountInfo::default().with_balance(U256::from(100)));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: existing, amount: 1 },
            Withdrawal { index: 1, validator_index: 1, address: new, amount: 2 },
            Withdrawal { index: 2, validator_index: 2, address: new, amount: 3 },
        ];

        let output = execute_block(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("EVM execution succeeds");

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.account_state(&existing).unwrap().info.as_ref().unwrap().balance,
            U256::from(1_000_000_100)
        );
        assert_eq!(
            output.account_state(&new).unwrap().info.as_ref().unwrap().balance,
            U256::from(5_000_000_000u64)
        );
    }

    #[test]
    fn zero_withdrawals_apply_eip161_state_clearing() {
        let nonexistent = address!("0000000000000000000000000000000000000001");
        let empty = address!("0000000000000000000000000000000000000002");
        let contract = address!("0000000000000000000000000000000000000003");
        let mut database = InMemoryDB::default();
        database.insert_account_info(&empty, AccountInfo::default());
        database.insert_account_info(&contract, AccountInfo::default().with_nonce(1));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: nonexistent, amount: 0 },
            Withdrawal { index: 1, validator_index: 1, address: empty, amount: 0 },
            Withdrawal { index: 2, validator_index: 2, address: contract, amount: 0 },
        ];

        let output = execute_block(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database.clone(),
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("EVM execution succeeds");

        assert!(output.account_state(&nonexistent).is_none());
        assert!(output.account_state(&empty).unwrap().info.is_none());
        assert!(output.account_state(&contract).is_none());
        let factory = crate::EthBlockExecutorFactory::new(MAINNET.clone());
        let env = crate::EthEvmEnv::new(SpecId::AMSTERDAM, BlockEnv::default(), 1);
        let mut evm = factory.evm_with_env(evm2::evm::Db::new(database), env);
        evm.state_mut().enable_bal_builder();
        post_block_balance_state_changes(
            &mut evm,
            &mut BlockState::new(),
            false,
            &mut |_| {},
            SpecId::AMSTERDAM,
            None,
            1,
            Address::ZERO,
            None,
            Some(&withdrawals),
        )
        .unwrap();
        let bal = evm.state_mut().take_bal_builder().unwrap();
        for withdrawal in withdrawals {
            assert!(bal.accounts.contains_key(&withdrawal.address));
        }
    }

    #[test]
    fn test_calc_base_block_reward() {
        // ((block number, td), reward)
        let cases = [
            // Pre-byzantium
            ((0, U256::ZERO), Some(ETH_TO_WEI * 5)),
            // Byzantium
            ((4370000, U256::ZERO), Some(ETH_TO_WEI * 3)),
            // Petersburg
            ((7280000, U256::ZERO), Some(ETH_TO_WEI * 2)),
            // Merge
            ((15537394, U256::from(58_750_000_000_000_000_000_000_u128)), None),
        ];

        for ((block_number, _td), expected_reward) in cases {
            assert_eq!(base_block_reward(&*MAINNET, block_number), expected_reward);
        }
    }

    #[test]
    fn test_calc_full_block_reward() {
        let base_reward = ETH_TO_WEI;
        let one_thirty_twoth_reward = base_reward >> 5;

        // (num_ommers, reward)
        let cases = [
            (0, base_reward),
            (1, base_reward + one_thirty_twoth_reward),
            (2, base_reward + one_thirty_twoth_reward * 2),
        ];

        for (num_ommers, expected_reward) in cases {
            assert_eq!(block_reward(base_reward, num_ommers), expected_reward);
        }
    }

    #[test]
    fn block_reward_uses_constantinople_activation() {
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(EthereumHardfork::Byzantium, ForkCondition::Block(9))
            .with_fork(EthereumHardfork::Constantinople, ForkCondition::Block(12))
            .with_fork(EthereumHardfork::Petersburg, ForkCondition::Block(15))
            .with_fork(EthereumHardfork::Paris, ForkCondition::Never)
            .build();

        assert_eq!(base_block_reward(&chain_spec, 11), Some(ETH_TO_WEI * 3));
        let reward = base_block_reward(&chain_spec, 12).unwrap();
        assert_eq!(reward, ETH_TO_WEI * 2);
        assert_eq!(block_reward(reward, 1), ETH_TO_WEI * 2 + ((ETH_TO_WEI * 2) >> 5));
        assert_eq!(ommer_reward(reward, 12, 11), ETH_TO_WEI * 7 / 4);
        assert_eq!(base_block_reward(&chain_spec, 14), Some(ETH_TO_WEI * 2));
    }

    #[test]
    fn collects_post_execution_system_call_requests() {
        let mut database = InMemoryDB::default();
        database.insert_account_info(
            &WITHDRAWAL_REQUEST_ADDRESS,
            AccountInfo::default().with_code(return_byte_code(0xaa)),
        );
        database.insert_account_info(
            &CONSOLIDATION_REQUEST_ADDRESS,
            AccountInfo::default().with_code(return_byte_code(0xbb)),
        );

        let output = execute_block(
            SpecId::PRAGUE,
            BlockEnv::default(),
            database,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            None,
        )
        .expect("system calls succeed");

        assert_eq!(
            output.result.requests.iter().cloned().collect::<Vec<_>>(),
            vec![
                Bytes::from_static(&[WITHDRAWAL_REQUEST_TYPE, 0xaa]),
                Bytes::from_static(&[CONSOLIDATION_REQUEST_TYPE, 0xbb]),
            ]
        );
        assert!(output.result.receipts.is_empty());
    }

    #[test]
    fn collects_amsterdam_builder_requests() {
        let mut database = InMemoryDB::default();
        for (address, byte) in [
            (WITHDRAWAL_REQUEST_ADDRESS, 0xaa),
            (CONSOLIDATION_REQUEST_ADDRESS, 0xbb),
            (BUILDER_DEPOSIT_REQUEST_ADDRESS, 0xcc),
            (BUILDER_EXIT_REQUEST_ADDRESS, 0xdd),
        ] {
            database.insert_account_info(
                &address,
                AccountInfo::default().with_code(return_byte_code(byte)),
            );
        }

        let output = execute_block(
            SpecId::AMSTERDAM,
            BlockEnv::default(),
            database,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            None,
        )
        .expect("system calls succeed");

        assert_eq!(
            output.result.requests.iter().cloned().collect::<Vec<_>>(),
            vec![
                Bytes::from_static(&[WITHDRAWAL_REQUEST_TYPE, 0xaa]),
                Bytes::from_static(&[CONSOLIDATION_REQUEST_TYPE, 0xbb]),
                Bytes::from_static(&[BUILDER_DEPOSIT_REQUEST_TYPE, 0xcc]),
                Bytes::from_static(&[BUILDER_EXIT_REQUEST_TYPE, 0xdd]),
            ]
        );
    }

    #[test]
    fn system_call_failures_follow_contract_policy() {
        for address in [
            BEACON_ROOTS_ADDRESS,
            HISTORY_STORAGE_ADDRESS,
            WITHDRAWAL_REQUEST_ADDRESS,
            CONSOLIDATION_REQUEST_ADDRESS,
        ] {
            for code in [
                Bytes::from_static(&[op::PUSH0, op::PUSH0, op::REVERT]),
                Bytes::from_static(&[op::INVALID]),
            ] {
                let mut database = InMemoryDB::default();
                for request_contract in [WITHDRAWAL_REQUEST_ADDRESS, CONSOLIDATION_REQUEST_ADDRESS]
                {
                    database.insert_account_info(
                        &request_contract,
                        AccountInfo::default()
                            .with_code(Bytecode::new_legacy(Bytes::from_static(&[op::STOP]))),
                    );
                }
                database.insert_account_info(
                    &address,
                    AccountInfo::default().with_code(Bytecode::new_legacy(code)),
                );
                let result = execute_block(
                    SpecId::PRAGUE,
                    BlockEnv { number: U256::from(1), ..Default::default() },
                    database,
                    core::iter::empty::<Recovered<TransactionSigned>>(),
                    None,
                );
                if matches!(address, BEACON_ROOTS_ADDRESS | HISTORY_STORAGE_ADDRESS) {
                    result.unwrap();
                } else {
                    let error = result.unwrap_err();
                    assert!(error.as_validation().is_some(), "{error:?}");
                }
            }
        }
    }

    #[test]
    fn collects_deposit_requests_from_transaction_logs() {
        let deposit = DepositEvent {
            pubkey: Bytes::from(vec![0x11; 48]),
            withdrawal_credentials: Bytes::from(vec![0x22; 32]),
            amount: Bytes::from(vec![0x33; 8]),
            signature: Bytes::from(vec![0x44; 96]),
            index: Bytes::from(vec![0x55; 8]),
        };
        let log = DepositEvent::encode_log(&Log {
            address: MAINNET_DEPOSIT_CONTRACT_ADDRESS,
            data: deposit,
        });
        let receipt = Receipt { tx_type: TxType::Legacy, logs: vec![log], ..Default::default() };

        let requests = block_requests_from_receipts(
            SpecId::PRAGUE,
            BlockExecutionContext {
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
            &[receipt],
        )
        .expect("deposit log decodes");

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][0], DEPOSIT_REQUEST_TYPE);
        assert_eq!(requests[0].len(), 1 + DEPOSIT_BYTES_SIZE);
    }

    fn return_byte_code(value: u8) -> Bytecode {
        Bytecode::new_legacy(Bytes::from(vec![
            op::PUSH1,
            value,
            op::PUSH1,
            0,
            op::MSTORE8,
            op::PUSH1,
            1,
            op::PUSH1,
            0,
            op::RETURN,
        ]))
    }
}
