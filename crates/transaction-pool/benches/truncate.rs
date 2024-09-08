#![allow(missing_docs)]
use alloy_primitives::{hex_literal::hex, Address};
use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use pprof::criterion::{Output, PProfProfiler};
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{RngAlgorithm, TestRng, TestRunner},
};
use reth_transaction_pool::{
    pool::{BasefeeOrd, ParkedPool, PendingPool, QueuedOrd},
    test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
    SubPoolLimit,
};

// constant seed to use for the rng
const SEED: [u8; 32] = hex!("1337133713371337133713371337133713371337133713371337133713371337");

/// Generates a set of `depth` dependent transactions, with the specified sender. Its values are
/// generated using [Arbitrary].
fn create_transactions_for_sender(
    mut runner: TestRunner,
    sender: Address,
    depth: usize,
) -> Vec<MockTransaction> {
    // TODO: for blob truncate, this would need a flag for _only_ generating 4844 mock transactions

    // assert that depth is always greater than zero, since empty vecs do not really make sense in
    // this context
    assert!(depth > 0);

    // make sure these are all post-eip-1559 transactions
    let mut txs = prop::collection::vec(any::<MockTransaction>(), depth)
        .new_tree(&mut runner)
        .unwrap()
        .current();

    for (nonce, tx) in txs.iter_mut().enumerate() {
        // reject pre-eip1559 tx types, if there is a legacy tx, replace it with an eip1559 tx
        if tx.is_legacy() || tx.is_eip2930() {
            *tx = MockTransaction::eip1559();

            // set fee values using arbitrary
            tx.set_priority_fee(any::<u128>().new_tree(&mut runner).unwrap().current());
            tx.set_max_fee(any::<u128>().new_tree(&mut runner).unwrap().current());
        }

        tx.set_sender(sender);
        tx.set_nonce(nonce as u64);
    }

    txs
}

/// Generates many transactions, each with a different sender. The number of transactions per
/// sender is generated using [Arbitrary]. The number of senders is specified by `senders`.
///
/// Because this uses [Arbitrary], the number of transactions per sender needs to be bounded. This
/// is done by using the `max_depth` parameter.
///
/// This uses [`create_transactions_for_sender`] to generate the transactions.
fn generate_many_transactions(senders: usize, max_depth: usize) -> Vec<MockTransaction> {
    let config = ProptestConfig::default();
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &SEED);
    let mut runner = TestRunner::new_with_rng(config, rng);

    let mut txs = Vec::new();
    for idx in 0..senders {
        // modulo max_depth so we know it is bounded, plus one so the minimum is always 1
        let depth = any::<usize>().new_tree(&mut runner).unwrap().current() % max_depth + 1;

        // set sender to an Address determined by the sender index. This should make any necessary
        // debugging easier.
        let idx_slice = idx.to_be_bytes();

        // pad with 12 bytes of zeros before rest
        let addr_slice = [0u8; 12].into_iter().chain(idx_slice.into_iter()).collect::<Vec<_>>();

        let sender = Address::from_slice(&addr_slice);
        txs.extend(create_transactions_for_sender(runner.clone(), sender, depth));
    }

    txs
}

/// Benchmarks all pool types for the truncate function.
fn benchmark_pools(group: &mut BenchmarkGroup<'_, WallTime>, senders: usize, max_depth: usize) {
    println!("Generating transactions for benchmark with {senders} unique senders and a max depth of {max_depth}...");
    let txs = generate_many_transactions(senders, max_depth);

    // benchmark parked pool
    truncate_basefee(group, "BasefeePool", txs.clone(), senders, max_depth);

    // benchmark pending pool
    truncate_pending(group, "PendingPool", txs.clone(), senders, max_depth);

    // benchmark queued pool
    truncate_queued(group, "QueuedPool", txs, senders, max_depth);

    // TODO: benchmark blob truncate
}

fn txpool_truncate(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transaction Pool Truncate");

    // the first few benchmarks (5, 10, 20, 100) should cause the txpool to hit the max tx limit,
    // so they are there to make sure we do not regress on best-case performance.
    //
    // the last few benchmarks (1000, 2000) should hit the max tx limit, at least for large enough
    // depth, so these should benchmark closer to real-world performance
    for senders in [5, 10, 20, 100, 1000, 2000] {
        // the max we'll be benching is 20, because MAX_ACCOUNT_SLOTS so far is 16. So 20 should be
        // a reasonable worst-case benchmark
        for max_depth in [1, 5, 10, 20] {
            benchmark_pools(&mut group, senders, max_depth);
        }
    }

    let large_senders = 5000;
    let max_depth = 16;

    // let's run a benchmark that includes a large number of senders and max_depth of 16 to ensure
    // we hit the TXPOOL_SUBPOOL_MAX_TXS_DEFAULT limit, which is currently 10k
    benchmark_pools(&mut group, large_senders, max_depth);

    // now we'll run a more realistic benchmark, with max depth of 1 and 15000 senders
    let realistic_senders = 15000;
    let realistic_max_depth = 1;
    benchmark_pools(&mut group, realistic_senders, realistic_max_depth);
}

fn truncate_pending(
    group: &mut BenchmarkGroup<'_, WallTime>,
    description: &str,
    seed: Vec<MockTransaction>,
    senders: usize,
    max_depth: usize,
) {
    let setup = || {
        let mut txpool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        for tx in &seed {
            // add transactions with a basefee of zero, so they are not immediately removed
            txpool.add_transaction(f.validated_arc(tx.clone()), 0);
        }
        txpool
    };

    let group_id = format!(
        "txpool | total txs: {} | total senders: {} | max depth: {} | {}",
        seed.len(),
        senders,
        max_depth,
        description,
    );

    // for now we just use the default SubPoolLimit
    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |mut txpool| {
            txpool.truncate_pool(SubPoolLimit::default());
            std::hint::black_box(());
        });
    });
}

fn truncate_queued(
    group: &mut BenchmarkGroup<'_, WallTime>,
    description: &str,
    seed: Vec<MockTransaction>,
    senders: usize,
    max_depth: usize,
) {
    let setup = || {
        let mut txpool = ParkedPool::<QueuedOrd<_>>::default();
        let mut f = MockTransactionFactory::default();

        for tx in &seed {
            txpool.add_transaction(f.validated_arc(tx.clone()));
        }
        txpool
    };

    let group_id = format!(
        "txpool | total txs: {} | total senders: {} | max depth: {} | {}",
        seed.len(),
        senders,
        max_depth,
        description,
    );

    // for now we just use the default SubPoolLimit
    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |mut txpool| {
            txpool.truncate_pool(SubPoolLimit::default());
            std::hint::black_box(());
        });
    });
}

fn truncate_basefee(
    group: &mut BenchmarkGroup<'_, WallTime>,
    description: &str,
    seed: Vec<MockTransaction>,
    senders: usize,
    max_depth: usize,
) {
    let setup = || {
        let mut txpool = ParkedPool::<BasefeeOrd<_>>::default();
        let mut f = MockTransactionFactory::default();

        for tx in &seed {
            txpool.add_transaction(f.validated_arc(tx.clone()));
        }
        txpool
    };

    let group_id = format!(
        "txpool | total txs: {} | total senders: {} | max depth: {} | {}",
        seed.len(),
        senders,
        max_depth,
        description,
    );

    // for now we just use the default SubPoolLimit
    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |mut txpool| {
            txpool.truncate_pool(SubPoolLimit::default());
            std::hint::black_box(());
        });
    });
}

criterion_group! {
    name = truncate;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = txpool_truncate
}
criterion_main!(truncate);
