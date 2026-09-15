#![allow(missing_docs)]

//! Benchmarks for transaction pool operations when the pool is at capacity and inserts
//! continuously trigger evictions, mimicking a saturated mainnet mempool.

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use reth_execution_types::ChangedAccount;
use reth_primitives_traits::SealedBlock;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore,
    noop::MockTransactionValidator,
    pool::PoolInner,
    test_utils::{MockOrdering, MockTransaction},
    validate::ValidTransaction,
    CanonicalStateUpdate, PoolConfig, PoolUpdateKind, SubPoolLimit, TransactionOrigin,
    TransactionValidationOutcome,
};

type BenchPool =
    PoolInner<MockTransactionValidator<MockTransaction>, MockOrdering, InMemoryBlobStore>;

/// Base fee the pool is initialized with.
const BASE_FEE: u64 = 100;

/// Average mock transaction size in bytes, so size-based limits behave realistically.
const TX_SIZE: usize = 640;

/// Deterministic unique sender address derived from a counter.
fn sender_address(counter: &mut u64) -> Address {
    *counter += 1;
    let mut bytes = [0u8; 20];
    bytes[12..].copy_from_slice(&counter.to_be_bytes());
    Address::from(bytes)
}

/// Creates a validated mock EIP-1559 transaction.
fn make_tx(sender: Address, nonce: u64, max_fee: u128, tip: u128) -> MockTransaction {
    let mut tx = MockTransaction::eip1559();
    tx.set_sender(sender)
        .set_nonce(nonce)
        .set_max_fee(max_fee)
        .set_priority_fee(tip)
        .set_gas_limit(100_000)
        .set_size(TX_SIZE);
    tx
}

/// Wraps a mock transaction into a validation outcome accepted by
/// [`PoolInner::add_transactions`].
const fn outcome(
    tx: MockTransaction,
    state_nonce: u64,
) -> TransactionValidationOutcome<MockTransaction> {
    TransactionValidationOutcome::Valid {
        balance: U256::MAX,
        state_nonce,
        bytecode_hash: None,
        transaction: ValidTransaction::Valid(tx),
        propagate: false,
        authorities: None,
    }
}

/// Builds a batch of gapless transactions for `senders` fresh senders with `txs_per_sender`
/// transactions each, all eligible for the pending pool.
fn pending_batch(
    counter: &mut u64,
    senders: usize,
    txs_per_sender: u64,
) -> Vec<(TransactionOrigin, TransactionValidationOutcome<MockTransaction>)> {
    let mut batch = Vec::with_capacity(senders * txs_per_sender as usize);
    for s in 0..senders {
        let sender = sender_address(counter);
        // spread fees so eviction has to make non-trivial ordering decisions
        let fee = (BASE_FEE as u128) + (s % 200) as u128;
        for nonce in 0..txs_per_sender {
            batch.push((
                TransactionOrigin::External,
                outcome(make_tx(sender, nonce, fee, 1 + (s % 50) as u128), 0),
            ));
        }
    }
    batch
}

/// Returns the sealed tip block used for canonical state updates.
fn tip_block() -> SealedBlock<reth_ethereum_primitives::Block> {
    let mut block = reth_ethereum_primitives::Block::default();
    block.header.gas_limit = 30_000_000;
    block.header.base_fee_per_gas = Some(BASE_FEE);
    SealedBlock::seal_slow(block)
}

/// Builds a saturated pool: pending pool at its limit, basefee and queued pools populated.
fn build_saturated_pool() -> (BenchPool, u64) {
    let pool = PoolInner::new(
        MockTransactionValidator::default(),
        MockOrdering::default(),
        InMemoryBlobStore::default(),
        PoolConfig::default(),
    );

    let block = tip_block();
    // initialize the pool's fee state
    pool.on_canonical_state_change(CanonicalStateUpdate {
        new_tip: &block,
        pending_block_base_fee: BASE_FEE,
        pending_block_blob_fee: None,
        changed_accounts: Vec::new(),
        mined_transactions: Vec::new(),
        update_kind: PoolUpdateKind::Commit,
    });

    let mut counter = 0u64;

    // pending: 2800 senders x 4 txs = 11200 txs, exceeds the default 10k pending limit
    let batch = pending_batch(&mut counter, 2800, 4);
    pool.add_transactions_with_origins(batch);

    // basefee parked: 800 senders x 4 txs below base fee
    let mut batch = Vec::new();
    for s in 0..800usize {
        let sender = sender_address(&mut counter);
        let fee = 8 + (s % 90) as u128;
        for nonce in 0..4u64 {
            batch.push((TransactionOrigin::External, outcome(make_tx(sender, nonce, fee, 1), 0)));
        }
    }
    pool.add_transactions_with_origins(batch);

    // queued: 400 senders x 4 txs with a nonce gap
    let mut batch = Vec::new();
    for s in 0..400usize {
        let sender = sender_address(&mut counter);
        let fee = (BASE_FEE as u128) + (s % 100) as u128;
        for nonce in 1..5u64 {
            batch.push((TransactionOrigin::External, outcome(make_tx(sender, nonce, fee, 1), 0)));
        }
    }
    pool.add_transactions_with_origins(batch);

    (pool, counter)
}

/// Builds an uncapped pending pool with one transaction per sender.
fn build_single_tx_sender_pool(senders: usize) -> (BenchPool, u64) {
    let pool = PoolInner::new(
        MockTransactionValidator::default(),
        MockOrdering::default(),
        InMemoryBlobStore::default(),
        PoolConfig { pending_limit: SubPoolLimit::max(), ..Default::default() },
    );

    let block = tip_block();
    pool.on_canonical_state_change(CanonicalStateUpdate {
        new_tip: &block,
        pending_block_base_fee: BASE_FEE,
        pending_block_blob_fee: None,
        changed_accounts: Vec::new(),
        mined_transactions: Vec::new(),
        update_kind: PoolUpdateKind::Commit,
    });

    let mut counter = 0;
    pool.add_transactions_with_origins(pending_batch(&mut counter, senders, 1));
    (pool, counter)
}

/// Returns all live senders followed by an additional 25% of tracked senders without
/// transactions.
fn senders_for_update_bench(pool: &BenchPool, counter: &mut u64) -> (Vec<Address>, usize) {
    let mut senders = pool.unique_senders().into_iter().collect::<Vec<_>>();
    senders.sort_unstable();
    let pool_senders = senders.len();

    let extra_senders = (0..pool_senders / 4).map(|_| sender_address(counter)).collect::<Vec<_>>();
    let _ = pool.get_sender_ids(extra_senders.iter().copied());
    senders.extend(extra_senders);

    (senders, pool_senders)
}

/// Benchmarks adding a batch of transactions to a pool at capacity, which forces eviction.
fn bench_add_transactions(c: &mut Criterion) {
    let mut group = c.benchmark_group("saturated_pool");
    group.sample_size(30);

    let (pool, mut counter) = build_saturated_pool();
    let size = pool.size();
    assert!(size.pending >= 9_000, "pending pool not saturated: {size:?}");

    group.bench_function("add_transactions_128_at_capacity", |b| {
        b.iter_batched(
            || {
                // 24 fresh senders x 4 pending txs + 32 underpriced parked txs
                let mut batch = pending_batch(&mut counter, 24, 4);
                for s in 0..32usize {
                    let sender = sender_address(&mut counter);
                    batch.push((
                        TransactionOrigin::External,
                        outcome(make_tx(sender, 0, 8 + (s % 90) as u128, 1), 0),
                    ));
                }
                batch
            },
            |batch| pool.add_transactions_with_origins(batch),
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

/// Benchmarks an account-only update against a pool at capacity: sender nonces and balances move
/// without any fee change, as happens on the reorg account-reload path and via
/// `Pool::update_accounts`.
fn bench_update_accounts(c: &mut Criterion) {
    let mut group = c.benchmark_group("saturated_pool");
    group.sample_size(30);

    let (pool, mut counter) = build_saturated_pool();
    let size = pool.size();
    assert!(size.pending >= 9_000, "pending pool not saturated: {size:?}");

    let (senders, pool_senders) = senders_for_update_bench(&pool, &mut counter);
    assert_eq!(pool_senders, 4_000);

    // the reported nonce stays put so the transactions are re-evaluated rather than discarded,
    // which keeps every iteration doing the same amount of work
    let mut flip = false;
    for ratio_percent in [0.25, 1.0, 10.0, 25.0, 50.0, 100.0, 125.0] {
        let changed_senders =
            ((pool_senders as f64 * ratio_percent / 100.0).round() as usize).max(1);
        group.bench_with_input(
            BenchmarkId::new("update_accounts", format!("{ratio_percent}%")),
            &changed_senders,
            |b, &changed_senders| {
                b.iter_batched(
                    || {
                        flip = !flip;
                        let balance =
                            if flip { U256::from(u128::MAX) } else { U256::from(u128::MAX - 1) };
                        senders[..changed_senders]
                            .iter()
                            .map(|address| ChangedAccount { address: *address, nonce: 0, balance })
                            .collect::<Vec<_>>()
                    },
                    |changed| pool.update_accounts(changed),
                    BatchSize::PerIteration,
                )
            },
        );
    }

    group.finish();
}

/// Benchmarks account-only updates by changed-sender ratio in pools where every sender has a
/// single transaction.
fn bench_update_accounts_single_tx_senders(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_tx_sender_pool");
    group.sample_size(30);

    for pool_senders in [100, 500, 1_000, 4_000] {
        let (pool, mut counter) = build_single_tx_sender_pool(pool_senders);
        let (senders, actual_pool_senders) = senders_for_update_bench(&pool, &mut counter);
        assert_eq!(actual_pool_senders, pool_senders);

        let mut flip = false;
        for ratio_percent in [25usize, 100, 125] {
            let changed_senders = pool_senders * ratio_percent / 100;
            group.bench_with_input(
                BenchmarkId::new(format!("{pool_senders}_senders"), format!("{ratio_percent}%")),
                &changed_senders,
                |b, &changed_senders| {
                    b.iter_batched(
                        || {
                            flip = !flip;
                            let balance = if flip {
                                U256::from(u128::MAX)
                            } else {
                                U256::from(u128::MAX - 1)
                            };
                            senders[..changed_senders]
                                .iter()
                                .map(|address| ChangedAccount {
                                    address: *address,
                                    nonce: 0,
                                    balance,
                                })
                                .collect::<Vec<_>>()
                        },
                        |changed| pool.update_accounts(changed),
                        BatchSize::PerIteration,
                    )
                },
            );
        }
    }

    group.finish();
}

/// Benchmarks a canonical state update against a pool at capacity: mined transactions are
/// removed, sender states change and the base fee moves.
fn bench_canonical_state_change(c: &mut Criterion) {
    let mut group = c.benchmark_group("saturated_pool");
    group.sample_size(30);

    let (pool, mut counter) = build_saturated_pool();
    let block = tip_block();

    let mut flip = false;

    group.bench_function("canonical_state_change_at_capacity", |b| {
        b.iter_batched(
            || {
                // refill with fresh transactions to keep the pool saturated (untimed)
                let batch = pending_batch(&mut counter, 25, 4);
                pool.add_transactions_with_origins(batch);

                // pick ~100 currently pending transactions as "mined"
                let pending = pool.pending_transactions();
                let mined = pending.iter().take(100).map(|tx| *tx.hash()).collect::<Vec<B256>>();

                // derive changed accounts from the mined transactions (highest nonce per sender)
                let mut changed: Vec<ChangedAccount> = Vec::new();
                for tx in pending.iter().take(100) {
                    let nonce = tx.nonce() + 1;
                    match changed.iter_mut().find(|acc| acc.address == tx.sender()) {
                        Some(acc) => acc.nonce = acc.nonce.max(nonce),
                        None => changed.push(ChangedAccount {
                            address: tx.sender(),
                            nonce,
                            balance: U256::MAX,
                        }),
                    }
                }

                // alternate the base fee by +/-12.5% to exercise promotions and demotions
                flip = !flip;
                let base_fee = if flip { BASE_FEE + BASE_FEE / 8 } else { BASE_FEE };

                (mined, changed, base_fee)
            },
            |(mined, changed, base_fee)| {
                pool.on_canonical_state_change(CanonicalStateUpdate {
                    new_tip: &block,
                    pending_block_base_fee: base_fee,
                    pending_block_blob_fee: None,
                    changed_accounts: changed,
                    mined_transactions: mined,
                    update_kind: PoolUpdateKind::Commit,
                })
            },
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_add_transactions,
    bench_canonical_state_change,
    bench_update_accounts,
    bench_update_accounts_single_tx_senders
);
criterion_main!(benches);
