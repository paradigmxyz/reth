use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_transaction_pool::test_utils::MockTransaction;

/// Transaction Pool trait for benching.
pub trait BenchTxPool: Default {
    fn add_transaction(&mut self, tx: MockTransaction);
    fn reorder(&mut self, base_fee: u64);
}

pub fn txpool_reordering(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transaction Pool Reordering");

    for seed_size in [1_000, 10_000, 50_000, 100_000] {
        for input_size in [10, 100, 1_000] {
            let (txs, new_txs, base_fee) = generate_test_data(seed_size, input_size);

            use implementations::*;

            // Vanilla sorting of unsorted collection
            txpool_reordering_bench::<VecTxPoolSortStable>(
                &mut group,
                "VecTxPoolSortStable",
                txs.clone(),
                new_txs.clone(),
                base_fee,
            );

            // Unstable sorting of unsorted collection
            txpool_reordering_bench::<VecTxPoolSortUnstable>(
                &mut group,
                "VecTxPoolSortUnstable",
                txs.clone(),
                new_txs.clone(),
                base_fee,
            );

            // BinaryHeap that is resorted on each update
            txpool_reordering_bench::<BinaryHeapTxPool>(
                &mut group,
                "BinaryHeapTxPool",
                txs,
                new_txs,
                base_fee,
            );
        }
    }
}

fn txpool_reordering_bench<T: BenchTxPool>(
    group: &mut BenchmarkGroup<WallTime>,
    description: &str,
    seed: Vec<MockTransaction>,
    new_txs: Vec<MockTransaction>,
    base_fee: u64,
) {
    let setup = || {
        let mut txpool = T::default();
        txpool.reorder(base_fee);

        for tx in seed.iter() {
            txpool.add_transaction(tx.clone());
        }
        (txpool, new_txs.clone())
    };

    let group_id = format!(
        "txpool | seed size: {} | input size: {} | {}",
        seed.len(),
        new_txs.len(),
        description
    );
    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |(mut txpool, new_txs)| {
            {
                // Reorder with new base fee
                let bigger_base_fee = base_fee.saturating_add(10);
                txpool.reorder(bigger_base_fee);

                // Reorder with new base fee after adding transactions.
                for new_tx in new_txs {
                    txpool.add_transaction(new_tx);
                }
                let smaller_base_fee = base_fee.saturating_sub(10);
                txpool.reorder(smaller_base_fee)
            };
            std::hint::black_box(());
        });
    });
}

fn generate_test_data(
    seed_size: usize,
    input_size: usize,
) -> (Vec<MockTransaction>, Vec<MockTransaction>, u64) {
    let config = ProptestConfig::default();
    let mut runner = TestRunner::new(config);

    let txs = prop::collection::vec(any::<MockTransaction>(), seed_size)
        .new_tree(&mut runner)
        .unwrap()
        .current();

    let new_txs = prop::collection::vec(any::<MockTransaction>(), input_size)
        .new_tree(&mut runner)
        .unwrap()
        .current();

    let base_fee = any::<u64>().new_tree(&mut runner).unwrap().current();

    (txs, new_txs, base_fee)
}

mod implementations {
    use super::*;
    use reth_transaction_pool::PoolTransaction;
    use std::collections::BinaryHeap;

    /// This implementation appends the transactions and uses [Vec::sort_by] function for sorting.
    #[derive(Default)]
    pub struct VecTxPoolSortStable {
        inner: Vec<MockTransaction>,
    }

    impl BenchTxPool for VecTxPoolSortStable {
        fn add_transaction(&mut self, tx: MockTransaction) {
            self.inner.push(tx);
        }

        fn reorder(&mut self, base_fee: u64) {
            self.inner.sort_by(|a, b| {
                a.effective_tip_per_gas(base_fee)
                    .expect("exists")
                    .cmp(&b.effective_tip_per_gas(base_fee).expect("exists"))
            })
        }
    }

    /// This implementation appends the transactions and uses [Vec::sort_unstable_by] function for
    /// sorting.
    #[derive(Default)]
    pub struct VecTxPoolSortUnstable {
        inner: Vec<MockTransaction>,
    }

    impl BenchTxPool for VecTxPoolSortUnstable {
        fn add_transaction(&mut self, tx: MockTransaction) {
            self.inner.push(tx);
        }

        fn reorder(&mut self, base_fee: u64) {
            self.inner.sort_unstable_by(|a, b| {
                a.effective_tip_per_gas(base_fee)
                    .expect("exists")
                    .cmp(&b.effective_tip_per_gas(base_fee).expect("exists"))
            })
        }
    }

    struct MockTransactionWithPriority {
        tx: MockTransaction,
        priority: u128,
    }

    impl PartialEq for MockTransactionWithPriority {
        fn eq(&self, other: &Self) -> bool {
            self.priority.eq(&other.priority)
        }
    }

    impl Eq for MockTransactionWithPriority {}

    impl PartialOrd for MockTransactionWithPriority {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for MockTransactionWithPriority {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.priority.cmp(&other.priority)
        }
    }

    /// This implementation uses BinaryHeap which is drained and reconstructed on each reordering.
    #[derive(Default)]
    pub struct BinaryHeapTxPool {
        inner: BinaryHeap<MockTransactionWithPriority>,
        base_fee: Option<u64>,
    }

    impl BenchTxPool for BinaryHeapTxPool {
        fn add_transaction(&mut self, tx: MockTransaction) {
            let priority = self
                .base_fee
                .as_ref()
                .map(|bf| tx.effective_tip_per_gas(*bf).expect("set"))
                .unwrap_or_default();
            self.inner.push(MockTransactionWithPriority { tx, priority });
        }

        fn reorder(&mut self, base_fee: u64) {
            self.base_fee = Some(base_fee);

            let drained = self.inner.drain();
            self.inner = BinaryHeap::from_iter(drained.map(|mock| {
                let priority = mock.tx.effective_tip_per_gas(base_fee).expect("set");
                MockTransactionWithPriority { tx: mock.tx, priority }
            }));
        }
    }
}

criterion_group!(reorder, txpool_reordering);
criterion_main!(reorder);
