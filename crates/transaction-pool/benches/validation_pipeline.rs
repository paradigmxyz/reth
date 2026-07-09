#![allow(missing_docs)]

//! Microbenchmarks for validation dispatch and the validation-to-insertion pipeline.

use alloy_primitives::{Address, B256};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use futures_util::future::join_all;
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::SignedTransaction;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore,
    noop::MockTransactionValidator,
    test_utils::{MockOrdering, MockTransaction, TransactionBuilder},
    validate::EthTransactionValidatorBuilder,
    AddedTransactionOutcome, EthPooledTransaction, EthTransactionValidator, Pool, PoolConfig,
    PoolResult, PoolTransaction, TransactionOrigin, TransactionPool, TransactionValidationOutcome,
    TransactionValidationTaskExecutor, TransactionValidator,
};
use std::hint::black_box;
use tokio::runtime::{Builder, Runtime};

type DirectPool = Pool<MockTransactionValidator<MockTransaction>, MockOrdering, InMemoryBlobStore>;
type TaskValidator = TransactionValidationTaskExecutor<MockTransactionValidator<MockTransaction>>;
type TaskPool = Pool<TaskValidator, MockOrdering, InMemoryBlobStore>;
type EthValidator = EthTransactionValidator<MockEthProvider, EthPooledTransaction, EthEvmConfig>;
type InsertResults = Vec<PoolResult<AddedTransactionOutcome>>;

fn runtime() -> Runtime {
    Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

fn make_transaction(id: u64) -> MockTransaction {
    make_transaction_for(id, id, 0)
}

fn make_transaction_for(id: u64, sender_id: u64, nonce: u64) -> MockTransaction {
    let mut hash = [0u8; 32];
    hash[24..].copy_from_slice(&id.to_be_bytes());
    let mut sender = [0u8; 20];
    sender[12..].copy_from_slice(&sender_id.to_be_bytes());

    MockTransaction::eip1559()
        .with_hash(B256::from(hash))
        .with_sender(Address::from(sender))
        .with_nonce(nonce)
        .with_gas_limit(21_000)
        .with_max_fee(1_000_000_000)
        .with_priority_fee(1_000_000)
        .with_size(128)
}

fn distinct_transactions(count: usize) -> Vec<MockTransaction> {
    (0..count).map(|index| make_transaction(index as u64 + 1)).collect()
}

fn same_sender_transactions(count: usize) -> Vec<MockTransaction> {
    (0..count).map(|index| make_transaction_for(index as u64 + 1, 1, index as u64)).collect()
}

fn make_eth_transaction(signer_id: u64, nonce: u64) -> EthPooledTransaction {
    let mut signer = [0u8; 32];
    signer[24..].copy_from_slice(&signer_id.max(1).to_be_bytes());
    let transaction = TransactionBuilder::default()
        .signer(B256::from(signer))
        .nonce(nonce)
        .gas_limit(100_000)
        .max_fee_per_gas(1_000_000_000)
        .max_priority_fee_per_gas(1_000_000)
        .into_eip1559()
        .try_into_recovered()
        .unwrap();
    EthPooledTransaction::try_from_consensus(transaction).unwrap()
}

fn eth_validation_fixture(
    count: usize,
    same_sender: bool,
) -> (EthValidator, Vec<EthPooledTransaction>) {
    let provider = MockEthProvider::default().with_genesis_block();
    let transactions = (0..count)
        .map(|index| {
            let signer_id = if same_sender { 1 } else { index as u64 + 1 };
            let nonce = if same_sender { index as u64 } else { 0 };
            let transaction = make_eth_transaction(signer_id, nonce);
            provider.add_account(
                transaction.sender(),
                ExtendedAccount::new(0, alloy_primitives::U256::MAX),
            );
            transaction
        })
        .collect();
    let validator = EthTransactionValidatorBuilder::new(provider, EthEvmConfig::mainnet())
        .build(InMemoryBlobStore::default());
    (validator, transactions)
}

fn direct_pool(config: PoolConfig) -> DirectPool {
    Pool::new(
        MockTransactionValidator::default(),
        MockOrdering::default(),
        InMemoryBlobStore::default(),
        config,
    )
}

fn task_pool(validator: TaskValidator) -> TaskPool {
    Pool::new(
        validator,
        MockOrdering::default(),
        InMemoryBlobStore::default(),
        PoolConfig::default(),
    )
}

fn task_validator(runtime: &Runtime) -> TaskValidator {
    // Keep channel handoff isolated from the production task pool. These are dispatch
    // microbenchmarks, not a model of production task scheduling.
    let (validator, task) =
        TransactionValidationTaskExecutor::new(MockTransactionValidator::default());
    runtime.spawn(task.run());
    validator
}

fn assert_valid<T: PoolTransaction>(label: &str, outcomes: &[TransactionValidationOutcome<T>]) {
    for outcome in outcomes {
        match outcome {
            TransactionValidationOutcome::Valid { .. } => {}
            TransactionValidationOutcome::Invalid(_, err) => {
                panic!("{label} returned an invalid outcome: {err:?}")
            }
            TransactionValidationOutcome::Error(_, err) => {
                panic!("{label} returned an error outcome: {err:?}")
            }
        }
    }
}

fn assert_inserted(results: &[PoolResult<AddedTransactionOutcome>]) {
    assert!(results.iter().all(Result::is_ok));
}

#[inline(never)]
fn insert_direct_batch(
    runtime: &Runtime,
    pool: DirectPool,
    transactions: Vec<MockTransaction>,
) -> (DirectPool, InsertResults) {
    let results =
        runtime.block_on(pool.add_transactions(TransactionOrigin::External, transactions));
    (pool, results)
}

#[inline(never)]
fn insert_task_batch(
    runtime: &Runtime,
    pool: TaskPool,
    transactions: Vec<MockTransaction>,
) -> (TaskPool, InsertResults) {
    let results =
        runtime.block_on(pool.add_transactions(TransactionOrigin::External, transactions));
    (pool, results)
}

fn bench_validation(c: &mut Criterion) {
    let runtime = runtime();
    let direct = MockTransactionValidator::default();
    let dispatched = task_validator(&runtime);
    let single_transaction = make_transaction(1);
    let (eth_single_validator, eth_single_transactions) = eth_validation_fixture(1, false);

    assert_valid(
        "direct single",
        &[runtime.block_on(
            direct.validate_transaction(TransactionOrigin::External, single_transaction.clone()),
        )],
    );
    assert_valid(
        "task single",
        &[runtime.block_on(
            dispatched
                .validate_transaction(TransactionOrigin::External, single_transaction.clone()),
        )],
    );
    assert_valid(
        "eth single",
        &[eth_single_validator
            .validate_one(TransactionOrigin::External, eth_single_transactions[0].clone())],
    );

    let mut group = c.benchmark_group("transaction_validation");
    group.throughput(Throughput::Elements(1));

    group.bench_function("direct_single", |b| {
        b.iter_batched(
            || single_transaction.clone(),
            |transaction| {
                black_box(runtime.block_on(
                    direct.validate_transaction(TransactionOrigin::External, transaction),
                ))
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("task_dispatch_single", |b| {
        b.iter_batched(
            || single_transaction.clone(),
            |transaction| {
                black_box(runtime.block_on(
                    dispatched.validate_transaction(TransactionOrigin::External, transaction),
                ))
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("eth_single", |b| {
        b.iter_batched(
            || eth_single_transactions[0].clone(),
            |transaction| {
                black_box(
                    eth_single_validator.validate_one(TransactionOrigin::External, transaction),
                )
            },
            BatchSize::SmallInput,
        )
    });

    for batch_size in [16usize, 128] {
        let transactions = distinct_transactions(batch_size);
        let (eth_distinct_validator, eth_distinct_transactions) =
            eth_validation_fixture(batch_size, false);
        let (eth_same_sender_validator, eth_same_sender_transactions) =
            eth_validation_fixture(batch_size, true);

        assert_valid(
            "direct batch",
            &runtime.block_on(direct.validate_transactions_with_origin(
                TransactionOrigin::External,
                transactions.clone(),
            )),
        );
        assert_valid(
            "task batch",
            &runtime.block_on(dispatched.validate_transactions_with_origin(
                TransactionOrigin::External,
                transactions.clone(),
            )),
        );
        assert_valid(
            "task concurrent batch",
            &runtime.block_on(join_all(transactions.iter().cloned().map(|transaction| {
                dispatched.validate_transaction(TransactionOrigin::External, transaction)
            }))),
        );
        assert_valid(
            "eth distinct batch",
            &runtime.block_on(eth_distinct_validator.validate_transactions_with_origin(
                TransactionOrigin::External,
                eth_distinct_transactions.clone(),
            )),
        );
        assert_valid(
            "eth same-sender batch",
            &runtime.block_on(eth_same_sender_validator.validate_transactions_with_origin(
                TransactionOrigin::External,
                eth_same_sender_transactions.clone(),
            )),
        );

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_function(format!("direct_batch_{batch_size}"), |b| {
            b.iter_batched(
                || transactions.clone(),
                |transactions| {
                    black_box(runtime.block_on(direct.validate_transactions_with_origin(
                        TransactionOrigin::External,
                        transactions,
                    )))
                },
                BatchSize::LargeInput,
            )
        });

        group.bench_function(format!("task_dispatch_batch_{batch_size}"), |b| {
            b.iter_batched(
                || transactions.clone(),
                |transactions| {
                    black_box(runtime.block_on(dispatched.validate_transactions_with_origin(
                        TransactionOrigin::External,
                        transactions,
                    )))
                },
                BatchSize::LargeInput,
            )
        });

        group.bench_function(format!("task_dispatch_concurrent_{batch_size}"), |b| {
            b.iter_batched(
                || transactions.clone(),
                |transactions| {
                    black_box(runtime.block_on(join_all(transactions.into_iter().map(
                        |transaction| {
                            dispatched
                                .validate_transaction(TransactionOrigin::External, transaction)
                        },
                    ))))
                },
                BatchSize::LargeInput,
            )
        });

        group.bench_function(format!("eth_batch_{batch_size}_distinct_senders"), |b| {
            b.iter_batched(
                || eth_distinct_transactions.clone(),
                |transactions| {
                    black_box(runtime.block_on(
                        eth_distinct_validator.validate_transactions_with_origin(
                            TransactionOrigin::External,
                            transactions,
                        ),
                    ))
                },
                BatchSize::LargeInput,
            )
        });

        group.bench_function(format!("eth_batch_{batch_size}_same_sender"), |b| {
            b.iter_batched(
                || eth_same_sender_transactions.clone(),
                |transactions| {
                    black_box(runtime.block_on(
                        eth_same_sender_validator.validate_transactions_with_origin(
                            TransactionOrigin::External,
                            transactions,
                        ),
                    ))
                },
                BatchSize::LargeInput,
            )
        });
    }

    group.finish();
}

fn bench_validation_to_insert(c: &mut Criterion) {
    let runtime = runtime();
    let dispatched = task_validator(&runtime);
    let single_transaction = make_transaction(1);

    let direct_preflight = direct_pool(PoolConfig::default());
    assert!(runtime
        .block_on(
            direct_preflight
                .add_transaction(TransactionOrigin::External, single_transaction.clone(),)
        )
        .is_ok());
    let task_preflight = task_pool(dispatched.clone());
    assert!(runtime
        .block_on(
            task_preflight
                .add_transaction(TransactionOrigin::External, single_transaction.clone(),)
        )
        .is_ok());

    let mut group = c.benchmark_group("validation_to_insert");
    group.throughput(Throughput::Elements(1));

    group.bench_function("direct_single_empty_pool", |b| {
        b.iter_batched(
            || (direct_pool(PoolConfig::default()), single_transaction.clone()),
            |(pool, transaction)| {
                let result = runtime
                    .block_on(pool.add_transaction(TransactionOrigin::External, transaction));
                black_box((pool, result))
            },
            BatchSize::LargeInput,
        )
    });

    group.bench_function("task_dispatch_single_empty_pool", |b| {
        b.iter_batched(
            || (task_pool(dispatched.clone()), single_transaction.clone()),
            |(pool, transaction)| {
                let result = runtime
                    .block_on(pool.add_transaction(TransactionOrigin::External, transaction));
                black_box((pool, result))
            },
            BatchSize::LargeInput,
        )
    });

    // 512 is a non-default stress point; the default per-account limit is 16 transactions.
    for batch_size in [16usize, 128, 512] {
        let transactions = distinct_transactions(batch_size);
        let same_sender = same_sender_transactions(batch_size);
        let same_sender_config = PoolConfig { max_account_slots: batch_size, ..Default::default() };

        let (preflight_pool, preflight_results) =
            insert_direct_batch(&runtime, direct_pool(PoolConfig::default()), transactions.clone());
        assert_inserted(&preflight_results);
        drop(preflight_pool);
        if batch_size <= 128 {
            let (preflight_pool, preflight_results) =
                insert_task_batch(&runtime, task_pool(dispatched.clone()), transactions.clone());
            assert_inserted(&preflight_results);
            drop(preflight_pool);
        }
        let (preflight_pool, preflight_results) = insert_direct_batch(
            &runtime,
            direct_pool(same_sender_config.clone()),
            same_sender.clone(),
        );
        assert_inserted(&preflight_results);
        drop(preflight_pool);

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_function(format!("direct_batch_{batch_size}_empty_pool"), |b| {
            b.iter_batched(
                || (direct_pool(PoolConfig::default()), transactions.clone()),
                |(pool, transactions)| black_box(insert_direct_batch(&runtime, pool, transactions)),
                BatchSize::LargeInput,
            )
        });

        if batch_size <= 128 {
            group.bench_function(format!("task_dispatch_batch_{batch_size}_empty_pool"), |b| {
                b.iter_batched(
                    || (task_pool(dispatched.clone()), transactions.clone()),
                    |(pool, transactions)| {
                        black_box(insert_task_batch(&runtime, pool, transactions))
                    },
                    BatchSize::LargeInput,
                )
            });
        }

        group.bench_function(format!("direct_same_sender_batch_{batch_size}_empty_pool"), |b| {
            b.iter_batched(
                || (direct_pool(same_sender_config.clone()), same_sender.clone()),
                |(pool, transactions)| black_box(insert_direct_batch(&runtime, pool, transactions)),
                BatchSize::LargeInput,
            )
        });
    }

    group.finish();
}

criterion_group!(benches, bench_validation, bench_validation_to_insert);
criterion_main!(benches);
