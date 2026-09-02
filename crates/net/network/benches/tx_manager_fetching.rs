#![allow(missing_docs)]

//! Benchmarks for the transaction fetching pipeline of the `TransactionsManager`, from
//! announcements to `GetPooledTransactions` requests and their responses.
//!
//! The workloads only use the manager's API, so they can be compared across fetcher
//! implementations.

use alloy_primitives::{
    map::{B256Map, B256Set},
    TxHash, B256,
};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use reth_eth_wire::{
    EthVersion, NewPooledTransactionHashes, NewPooledTransactionHashes68, PooledTransactions,
};
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_network::transactions::test_harness::{MockRequest, TxFetchHarness};
use reth_network_peers::PeerId;
use reth_transaction_pool::{test_utils::TransactionGenerator, TransactionPool};
use tokio::runtime::Runtime;

/// Announced size of every transaction.
const TX_SIZE: usize = 512;
/// Transaction type announced for every transaction.
const TX_TYPE: u8 = 2;

fn peer_id(index: usize) -> PeerId {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(index as u64 + 1).to_be_bytes());
    PeerId::new(bytes)
}

fn hash(index: usize) -> TxHash {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&(index as u64 + 1).to_be_bytes());
    B256::from(bytes)
}

fn announcement(hashes: &[TxHash]) -> NewPooledTransactionHashes {
    NewPooledTransactionHashes::Eth68(NewPooledTransactionHashes68 {
        types: vec![TX_TYPE; hashes.len()],
        sizes: vec![TX_SIZE; hashes.len()],
        hashes: hashes.to_vec(),
    })
}

/// Announcements every peer of the workload delivers.
struct Workload {
    peers: Vec<PeerId>,
    announcements: Vec<(PeerId, Vec<TxHash>)>,
}

impl Workload {
    /// Every peer announces its own slice of `hashes`.
    fn disjoint(num_peers: usize, hashes: &[TxHash]) -> Self {
        let peers = (0..num_peers).map(peer_id).collect::<Vec<_>>();
        let per_peer = hashes.len() / num_peers;
        let announcements = peers
            .iter()
            .enumerate()
            .map(|(i, peer_id)| (*peer_id, hashes[i * per_peer..(i + 1) * per_peer].to_vec()))
            .collect();
        Self { peers, announcements }
    }

    /// Every peer announces all `hashes`.
    fn gossip(num_peers: usize, hashes: &[TxHash]) -> Self {
        let peers = (0..num_peers).map(peer_id).collect::<Vec<_>>();
        let announcements = peers.iter().map(|peer_id| (*peer_id, hashes.to_vec())).collect();
        Self { peers, announcements }
    }

    fn harness(&self, runtime: &Runtime) -> TxFetchHarness {
        runtime.block_on(TxFetchHarness::new(self.peers.iter().copied(), EthVersion::Eth68))
    }

    fn announce(&self, harness: &mut TxFetchHarness) {
        for (peer_id, hashes) in &self.announcements {
            harness.announce(*peer_id, announcement(hashes));
        }
    }
}

/// Totals of the requests a run produced.
#[derive(Debug, Default, PartialEq, Eq)]
struct Stats {
    requests: usize,
    hashes: usize,
}

/// Delivers announcements, polls until the requests are sent and takes them.
fn announce_and_poll(workload: &Workload, harness: &mut TxFetchHarness) -> Vec<MockRequest> {
    workload.announce(harness);
    harness.poll_until_idle();
    harness.take_requests()
}

/// Delivers announcements and answers requests with `respond` until the manager stops sending
/// requests.
fn run_to_completion(
    workload: &Workload,
    harness: &mut TxFetchHarness,
    respond: impl Fn(&[TxHash]) -> PooledTransactions<PooledTransactionVariant>,
) -> Stats {
    workload.announce(harness);
    let mut stats = Stats::default();
    loop {
        harness.poll_until_idle();
        let requests = harness.take_requests();
        if requests.is_empty() {
            return stats
        }
        for request in requests {
            stats.requests += 1;
            stats.hashes += request.request.0.len();
            let _ = request.response.send(Ok(respond(&request.request.0)));
        }
    }
}

fn bench_announce(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let _enter = runtime.enter();
    let mut group = c.benchmark_group("tx_manager/announce");

    // 64 peers announce 64 hashes each
    let hashes = (0..4096).map(hash).collect::<Vec<_>>();
    let workload = Workload::disjoint(64, &hashes);
    let requests = announce_and_poll(&workload, &mut workload.harness(&runtime));
    assert_eq!(requests.len(), 64, "every peer is asked for its hashes");
    assert_eq!(requests.iter().map(|r| r.request.0.len()).sum::<usize>(), 4096);

    group.throughput(Throughput::Elements(4096));
    group.bench_function("disjoint", |b| {
        b.iter_batched_ref(
            || workload.harness(&runtime),
            |harness| announce_and_poll(&workload, harness),
            BatchSize::LargeInput,
        )
    });

    // 64 peers announce the same 2048 hashes
    let workload = Workload::gossip(64, &hashes[..2048]);
    let requests = announce_and_poll(&workload, &mut workload.harness(&runtime));
    let unique = requests.iter().flat_map(|r| r.request.0.iter()).copied().collect::<B256Set>();
    assert_eq!(unique.len(), requests.iter().map(|r| r.request.0.len()).sum::<usize>());

    group.throughput(Throughput::Elements(64 * 2048));
    group.bench_function("gossip", |b| {
        b.iter_batched_ref(
            || workload.harness(&runtime),
            |harness| announce_and_poll(&workload, harness),
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

fn bench_fetch(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let _enter = runtime.enter();
    let mut group = c.benchmark_group("tx_manager/fetch");

    // 3 peers announce the same 2048 hashes and none of them delivers, so every hash is retried
    // until the peers are exhausted
    let hashes = (0..2048).map(hash).collect::<Vec<_>>();
    let workload = Workload::gossip(3, &hashes);
    let stats = run_to_completion(&workload, &mut workload.harness(&runtime), |_| {
        PooledTransactions::default()
    });
    println!("gossip_empty_responses: {stats:?}");

    group.throughput(Throughput::Elements(2048));
    group.bench_function("gossip_empty_responses", |b| {
        b.iter_batched_ref(
            || workload.harness(&runtime),
            |harness| run_to_completion(&workload, harness, |_| PooledTransactions::default()),
            BatchSize::LargeInput,
        )
    });

    // 8 peers announce the same 512 transactions, every request is answered in full and the
    // transactions are imported into the pool
    // every transaction gets its own signer so the pool's per sender limits don't apply
    let mut generator = TransactionGenerator::with_num_signers(rand::rng(), 512);
    let txs = generator
        .signer_keys
        .clone()
        .into_iter()
        .map(|signer| {
            let mut tx = generator.transaction();
            tx.signer = signer;
            PooledTransactionVariant::try_from(tx.into_eip1559()).unwrap()
        })
        .collect::<Vec<_>>();
    let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
    assert_eq!(hashes.iter().copied().collect::<B256Set>().len(), hashes.len(), "unique txs");
    let by_hash = txs.into_iter().map(|tx| (*tx.tx_hash(), tx)).collect::<B256Map<_>>();
    let deliver = |requested: &[TxHash]| {
        PooledTransactions(requested.iter().map(|hash| by_hash[hash].clone()).collect())
    };
    let workload = Workload::gossip(8, &hashes);
    let mut harness = workload.harness(&runtime);
    let stats = run_to_completion(&workload, &mut harness, deliver);
    println!("gossip_delivered: {stats:?}");
    assert_eq!(harness.pool().get_all(hashes.clone()).len(), hashes.len(), "all txs imported");

    group.throughput(Throughput::Elements(512));
    group.bench_function("gossip_delivered", |b| {
        b.iter_batched_ref(
            || workload.harness(&runtime),
            |harness| run_to_completion(&workload, harness, deliver),
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

criterion_group!(benches, bench_announce, bench_fetch);
criterion_main!(benches);
