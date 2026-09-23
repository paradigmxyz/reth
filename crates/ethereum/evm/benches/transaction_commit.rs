//! Compare combined and detached transaction commits on the same storage-writing workload.

#![allow(missing_docs)]

use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_primitives::{Address, Bytes, Signature, TxKind, U256};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use evm2::{
    bytecode::Bytecode,
    env::BlockEnv,
    evm::{AccountInfo, InMemoryDB},
    interpreter::opcode::op,
    SpecId,
};
use reth_chainspec::ChainSpecBuilder;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{BlockExecutor, BlockExecutorFactory};
use reth_evm_ethereum::{EthBlockExecutionCtx, EthBlockExecutorFactory, EthEvmEnv};
use reth_primitives_traits::Recovered;
use std::{hint::black_box, sync::Arc};

fn transaction_commit(c: &mut Criterion) {
    const TRANSACTIONS: u64 = 64;
    let caller = Address::with_last_byte(0xaa);
    let contract = Address::with_last_byte(0xbb);
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        &caller,
        AccountInfo::default().with_balance(U256::from(1_000_000_000u64)),
    );
    db.insert_account_info(
        &contract,
        AccountInfo::default().with_nonce(1).with_code(Bytecode::new_raw(Bytes::from_static(&[
            op::PUSH1,
            0,
            op::SLOAD,
            op::PUSH1,
            1,
            op::ADD,
            op::PUSH1,
            0,
            op::SSTORE,
            op::STOP,
        ]))),
    );
    let transactions: Vec<_> = (0..TRANSACTIONS)
        .map(|nonce| {
            Recovered::new_unchecked(
                TransactionSigned::Legacy(
                    TxLegacy {
                        chain_id: Some(1),
                        nonce,
                        gas_limit: 100_000,
                        gas_price: 1,
                        to: TxKind::Call(contract),
                        ..Default::default()
                    }
                    .into_signed(Signature::test_signature()),
                ),
                caller,
            )
        })
        .collect();
    let factory = EthBlockExecutorFactory::new(Arc::new(
        ChainSpecBuilder::mainnet().london_activated().build(),
    ));
    let env = EthEvmEnv::new(
        SpecId::LONDON,
        BlockEnv::<evm2::BaseEvmTypes> { gas_limit: U256::from(10_000_000), ..Default::default() },
        1,
    );
    let ctx = EthBlockExecutionCtx {
        tx_count_hint: Some(TRANSACTIONS as usize),
        parent_hash: Default::default(),
        parent_beacon_block_root: None,
        ommers: &[],
        withdrawals: None,
        extra_data: Bytes::new(),
        slot_number: None,
    };
    let mut group = c.benchmark_group("transaction_commit");
    group.throughput(Throughput::Elements(TRANSACTIONS));
    for detached in [false, true] {
        group.bench_function(if detached { "detached" } else { "combined" }, |b| {
            b.iter_batched(
                || {
                    factory
                        .create_executor(factory.evm_with_env(db.clone(), env.clone()), ctx.clone())
                },
                |mut executor| {
                    for tx in &transactions {
                        if detached {
                            let output =
                                executor.execute_transaction_without_commit(tx.clone()).unwrap();
                            black_box(executor.commit_transaction(output).unwrap());
                        } else {
                            black_box(executor.execute_transaction(tx.clone()).unwrap());
                        }
                    }
                    let (output, _) = executor.finish_with_block_access_list().unwrap();
                    assert_eq!(
                        output.storage(&contract, U256::ZERO).unwrap(),
                        U256::from(TRANSACTIONS)
                    );
                    black_box(output);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, transaction_commit);
criterion_main!(benches);
