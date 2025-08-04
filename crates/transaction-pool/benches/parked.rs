use alloy_primitives::address;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_primitives_traits::InMemorySize;
use reth_transaction_pool::{
    identifier::{SenderId, TransactionId},
    test_utils::{MockTransaction, MockTransactionFactory, MockTransactionSet},
    PoolTransaction, SubPoolLimit, ValidPoolTransaction, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, BTreeMap, BTreeSet},
    ops::{Bound::Unbounded, Deref},
    sync::Arc,
};

// === Vec-based Implementation ===

#[derive(Debug, Clone)]
pub struct ParkedPoolVec<T: ParkedOrd> {
    submission_id: u64,
    by_id: BTreeMap<TransactionId, ParkedPoolTransaction<T>>,
    sender_id_count: Vec<SenderCount>,
    sender_id_last_submission: Vec<SenderSubmission>,
    size_of: SizeTracker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SenderCount(u128);

impl SenderCount {
    const fn new(sender_id: SenderId, count: u64) -> Self {
        Self(((sender_id.as_u64()) as u128) << 64 | count as u128)
    }
    const fn sender_id(self) -> SenderId {
        SenderId::from_u64((self.0 >> 64) as u64)
    }
    const fn count(self) -> u64 {
        self.0 as u64
    }
}

impl Ord for SenderCount {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sender_id().cmp(&other.sender_id()).then_with(|| self.count().cmp(&other.count()))
    }
}

impl PartialOrd for SenderCount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderSubmission(u128);

impl SenderSubmission {
    const fn new(sender_id: SenderId, submission_id: u64) -> Self {
        Self(((sender_id.as_u64()) as u128) << 64 | submission_id as u128)
    }
    const fn sender_id(self) -> SenderId {
        SenderId::from_u64((self.0 >> 64) as u64)
    }
    const fn submission_id(self) -> u64 {
        self.0 as u64
    }
}

impl Ord for SenderSubmission {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sender_id()
            .cmp(&other.sender_id())
            .then_with(|| other.submission_id().cmp(&self.submission_id()))
    }
}

impl PartialOrd for SenderSubmission {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ParkedOrd> ParkedPoolVec<T> {
    pub fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        let id = tx.transaction_id;
        assert!(!self.by_id.contains_key(&id), "transaction already included");
        let submission_id = self.next_id();
        self.size_of += tx.transaction.size();
        self.add_sender_count(tx.transaction_id.sender, submission_id);
        let transaction = ParkedPoolTransaction { submission_id, transaction: tx.into() };
        self.by_id.insert(id, transaction);
    }

    fn add_sender_count(&mut self, sender: SenderId, submission_id: u64) {
        let count_pos = self.sender_id_count.binary_search_by_key(&sender, |sc| sc.sender_id());
        let submission_pos =
            self.sender_id_last_submission.binary_search_by_key(&sender, |ss| ss.sender_id());
        match count_pos {
            Ok(idx) => {
                let current_count = self.sender_id_count[idx].count();
                self.sender_id_count[idx] = SenderCount::new(sender, current_count + 1);
                if let Ok(sub_idx) = submission_pos {
                    self.sender_id_last_submission[sub_idx] =
                        SenderSubmission::new(sender, submission_id);
                }
            }
            Err(idx) => {
                self.sender_id_count.insert(idx, SenderCount::new(sender, 1));
                let sub_idx = submission_pos.unwrap_err();
                self.sender_id_last_submission
                    .insert(sub_idx, SenderSubmission::new(sender, submission_id));
            }
        }
    }

    const fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }
}

impl<T: ParkedOrd> Default for ParkedPoolVec<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            by_id: Default::default(),
            sender_id_count: Default::default(),
            sender_id_last_submission: Default::default(),
            size_of: Default::default(),
        }
    }
}

// === HashMap/BTreeSet-based Implementation ===

#[derive(Debug, Clone)]
pub struct ParkedPoolHashMap<T: ParkedOrd> {
    submission_id: u64,
    by_id: BTreeMap<TransactionId, ParkedPoolTransaction<T>>,
    last_sender_submission: BTreeSet<SubmissionSenderId>,
    sender_transaction_count: FxHashMap<SenderId, SenderTransactionCount>,
    size_of: SizeTracker,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SenderTransactionCount {
    count: u64,
    last_submission_id: u64,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct SubmissionSenderId {
    pub sender_id: SenderId,
    pub submission_id: u64,
}

impl SubmissionSenderId {
    const fn new(sender_id: SenderId, submission_id: u64) -> Self {
        Self { sender_id, submission_id }
    }
}

impl Ord for SubmissionSenderId {
    fn cmp(&self, other: &Self) -> Ordering {
        other.submission_id.cmp(&self.submission_id)
    }
}

impl PartialOrd for SubmissionSenderId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ParkedOrd> ParkedPoolHashMap<T> {
    pub fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        let id = tx.transaction_id;
        assert!(!self.by_id.contains_key(&id), "transaction already included");
        let submission_id = self.next_id();
        self.size_of += tx.transaction.size();
        self.add_sender_count(id.sender, submission_id);
        let transaction = ParkedPoolTransaction { submission_id, transaction: tx.into() };
        self.by_id.insert(id, transaction);
    }

    fn add_sender_count(&mut self, sender: SenderId, submission_id: u64) {
        match self.sender_transaction_count.entry(sender) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();
                self.last_sender_submission
                    .remove(&SubmissionSenderId::new(sender, value.last_submission_id));
                value.count += 1;
                value.last_submission_id = submission_id;
            }
            Entry::Vacant(entry) => {
                entry
                    .insert(SenderTransactionCount { count: 1, last_submission_id: submission_id });
            }
        }
        self.last_sender_submission.insert(SubmissionSenderId::new(sender, submission_id));
    }

    const fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }
}

impl<T: ParkedOrd> Default for ParkedPoolHashMap<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            by_id: Default::default(),
            last_sender_submission: Default::default(),
            sender_transaction_count: Default::default(),
            size_of: Default::default(),
        }
    }
}

// === Shared Structures ===

#[derive(Debug)]
struct ParkedPoolTransaction<T: ParkedOrd> {
    submission_id: u64,
    transaction: T,
}

impl<T: ParkedOrd> Clone for ParkedPoolTransaction<T> {
    fn clone(&self) -> Self {
        Self { submission_id: self.submission_id, transaction: self.transaction.clone() }
    }
}

impl<T: ParkedOrd> Eq for ParkedPoolTransaction<T> {}

impl<T: ParkedOrd> PartialEq<Self> for ParkedPoolTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: ParkedOrd> PartialOrd<Self> for ParkedPoolTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ParkedOrd> Ord for ParkedPoolTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.transaction
            .cmp(&other.transaction)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

pub trait ParkedOrd:
    Ord
    + Clone
    + From<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Into<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Deref<Target = Arc<ValidPoolTransaction<Self::Transaction>>>
{
    type Transaction: PoolTransaction;
}

#[derive(Debug)]
pub struct BasefeeOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

macro_rules! impl_ord_wrapper {
    ($name:ident) => {
        impl<T: PoolTransaction> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }
        impl<T: PoolTransaction> Eq for $name<T> {}
        impl<T: PoolTransaction> PartialEq<Self> for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.cmp(other) == Ordering::Equal
            }
        }
        impl<T: PoolTransaction> PartialOrd<Self> for $name<T> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        impl<T: PoolTransaction> Deref for $name<T> {
            type Target = Arc<ValidPoolTransaction<T>>;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl<T: PoolTransaction> ParkedOrd for $name<T> {
            type Transaction = T;
        }
        impl<T: PoolTransaction> From<Arc<ValidPoolTransaction<T>>> for $name<T> {
            fn from(value: Arc<ValidPoolTransaction<T>>) -> Self {
                Self(value)
            }
        }
        impl<T: PoolTransaction> From<$name<T>> for Arc<ValidPoolTransaction<T>> {
            fn from(value: $name<T>) -> Arc<ValidPoolTransaction<T>> {
                value.0
            }
        }
    };
}

impl_ord_wrapper!(BasefeeOrd);

impl<T: PoolTransaction> Ord for BasefeeOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.transaction.max_fee_per_gas().cmp(&other.0.transaction.max_fee_per_gas())
    }
}

// === Benchmark Setup ===

fn bench_add_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("ParkedPool add_transaction");
    let sizes = vec![100, 1_000, 10_000];
    let mut f = MockTransactionFactory::default();

    for size in sizes {
        // Single sender: all transactions from one sender
        group.bench_with_input(
            BenchmarkId::new("Vec-based_single_sender", size),
            &size,
            |b, &size| {
                let sender = address!("0x000000000000000000000000000000000000000a");
                let txs = MockTransactionSet::dependent(
                    sender,
                    0,
                    size,
                    alloy_consensus::TxType::Eip1559,
                )
                .into_vec()
                .into_iter()
                .map(|tx| f.validated_arc(tx))
                .collect::<Vec<_>>();
                let mut pool = ParkedPoolVec::<BasefeeOrd<MockTransaction>>::default();
                b.iter(|| {
                    for tx in txs.iter() {
                        pool.add_transaction(black_box(tx.clone()));
                    }
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("HashMap-based_single_sender", size),
            &size,
            |b, &size| {
                let sender = address!("0x000000000000000000000000000000000000000a");
                let txs = MockTransactionSet::dependent(
                    sender,
                    0,
                    size,
                    alloy_consensus::TxType::Eip1559,
                )
                .into_vec()
                .into_iter()
                .map(|tx| f.validated_arc(tx))
                .collect::<Vec<_>>();
                let mut pool = ParkedPoolHashMap::<BasefeeOrd<MockTransaction>>::default();
                b.iter(|| {
                    for tx in txs.iter() {
                        pool.add_transaction(black_box(tx.clone()));
                    }
                });
            },
        );

        // Multiple senders: one transaction per sender
        group.bench_with_input(
            BenchmarkId::new("Vec-based_multi_sender", size),
            &size,
            |b, &size| {
                let txs = (0..size)
                    .map(|i| {
                        let addr = address!("0x0000000000000000000000000000000000000000").0;
                        let mut tx = MockTransaction::eip1559();
                        tx.set_sender(addr.into());
                        txs.push(f.validated_arc(tx));
                    })
                    .collect::<Vec<_>>();
                let mut pool = ParkedPoolVec::<BasefeeOrd<MockTransaction>>::default();
                b.iter(|| {
                    for tx in txs.iter() {
                        pool.add_transaction(tx.clone());
                    }
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("HashMap-based_multi_sender", size),
            &size,
            |b, &size| {
                let txs = (0..size)
                    .map(|i| {
                        let addr = address!("0x0000000000000000000000000000000000000000").0;
                        let mut tx = MockTransaction::eip1559();
                        tx.set_sender(addr.into());
                        txs.push(f.validated_arc(tx));
                    })
                    .collect::<Vec<_>>();
                let mut pool = ParkedPoolHashMap::<BasefeeOrd<MockTransaction>>::default();
                b.iter(|| {
                    for tx in txs.iter() {
                        pool.add_transaction(tx.clone());
                    }
                });
            },
        );

        // Mixed: half senders with multiple transactions, half with one
        group.bench_with_input(BenchmarkId::new("Vec-based_mixed", size), &size, |b, &size| {
            let half = size / 2;
            let mut txs = Vec::new();
            // Half senders with multiple transactions
            for i in 0..half {
                let addr = address!("0x0000000000000000000000000000000000000000").0;
                let sender_txs = MockTransactionSet::dependent(
                    addr.into(),
                    0,
                    2,
                    alloy_consensus::TxType::Eip1559,
                )
                .into_vec()
                .into_iter()
                .map(|tx| f.validated_arc(tx))
                .collect::<Vec<_>>();
                txs.extend(sender_txs);
            }
            // Half senders with one transaction
            for i in half..size {
                let addr = address!("0x0000000000000000000000000000000000000000").0;
                let mut tx = MockTransaction::eip1559();
                tx.set_sender(addr.into());
                txs.push(f.validated_arc(tx));
            }
            let mut pool = ParkedPoolVec::<BasefeeOrd<MockTransaction>>::default();
            b.iter(|| {
                for tx in txs.iter() {
                    pool.add_transaction(black_box(tx.clone()));
                }
            });
        });

        group.bench_with_input(BenchmarkId::new("HashMap-based_mixed", size), &size, |b, &size| {
            let half = size / 2;
            let mut txs = Vec::new();
            for i in 0..half {
                let addr = address!("0x0000000000000000000000000000000000000000").0;
                let sender_txs = MockTransactionSet::dependent(
                    addr.into(),
                    0,
                    2,
                    alloy_consensus::TxType::Eip1559,
                )
                .into_vec()
                .into_iter()
                .map(|tx| f.validated_arc(tx))
                .collect::<Vec<_>>();
                txs.extend(sender_txs);
            }
            for i in half..size {
                let addr = address!("0x0000000000000000000000000000000000000000").0;
                let mut tx = MockTransaction::eip1559();
                tx.set_sender(addr.into());
                txs.push(f.validated_arc(tx));
            }
            let mut pool = ParkedPoolHashMap::<BasefeeOrd<MockTransaction>>::default();
            b.iter(|| {
                for tx in txs.iter() {
                    pool.add_transaction(black_box(tx.clone()));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_add_transaction);
criterion_main!(benches);
