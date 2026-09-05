#![allow(missing_docs)]

//! Benchmarks for the `TransactionFetcher`: recording announcements from many peers, packing
//! `GetPooledTransactions` requests for idle peers and processing their responses.

use alloy_consensus::transaction::Recovered;
use alloy_primitives::{
    map::{B256Map, FbBuildHasher, HashMap},
    TxHash,
};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use futures::{task::noop_waker_ref, StreamExt};
use reth_eth_wire::{Eth68TxMetadata, EthVersion, PooledTransactions};
use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
use reth_network::{
    test_utils::transactions::new_mock_session_with_capacity,
    transactions::{
        constants::tx_fetcher::MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH,
        fetcher::TransactionFetcher, PeerMetadata,
    },
};
use reth_network_api::PeerRequest;
use reth_network_peers::PeerId;
use reth_transaction_pool::test_utils::MockTransactionFactory;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

/// Number of connected peers.
const PEERS: usize = 64;
/// Number of transactions announced in the gossip scenarios, one full announcement.
const TXS: usize = 4096;
/// Announced size of every transaction.
const TX_SIZE: usize = 512;
/// Transaction type announced for every transaction.
const TX_TYPE: u8 = 2;

/// A fetcher with mock peer sessions.
struct Rig {
    fetcher: TransactionFetcher,
    peers: HashMap<PeerId, PeerMetadata, FbBuildHasher<64>>,
    sessions: Vec<(PeerId, mpsc::Receiver<PeerRequest>)>,
}

impl Rig {
    fn new(num_peers: usize) -> Self {
        let mut peers = HashMap::default();
        let mut sessions = Vec::with_capacity(num_peers);
        for i in 0..num_peers {
            let peer_id = peer_id(i);
            let (peer, rx) = new_mock_session_with_capacity(peer_id, EthVersion::Eth68, 4);
            peers.insert(peer_id, peer);
            sessions.push((peer_id, rx));
        }
        Self { fetcher: TransactionFetcher::default(), peers, sessions }
    }

    /// Every peer announces the same hashes, like a gossiped batch of transactions.
    fn announce_gossip(&mut self, announcement: &[(TxHash, Eth68TxMetadata)]) {
        for (peer_id, _) in &self.sessions {
            self.fetcher.on_announcement(*peer_id, announcement.iter().copied());
        }
    }

    /// Every peer announces its own slice of the hashes.
    fn announce_disjoint(&mut self, announcement: &[(TxHash, Eth68TxMetadata)]) {
        let per_peer = announcement.len() / self.sessions.len();
        for (i, (peer_id, _)) in self.sessions.iter().enumerate() {
            let slice = &announcement[i * per_peer..(i + 1) * per_peer];
            self.fetcher.on_announcement(*peer_id, slice.iter().copied());
        }
    }

    fn dispatch(&mut self) -> usize {
        self.fetcher.dispatch(&self.peers, usize::MAX)
    }

    /// Answers every queued request with the given handler and processes the resulting events.
    fn respond_all(
        &mut self,
        respond: impl Fn(&[TxHash]) -> PooledTransactions<PooledTransactionVariant>,
    ) {
        for (_, rx) in &mut self.sessions {
            while let Ok(PeerRequest::GetPooledTransactions { request, response }) = rx.try_recv() {
                let _ = response.send(Ok(respond(&request.0)));
            }
        }
        let mut cx = Context::from_waker(noop_waker_ref());
        while let Poll::Ready(Some(_)) = self.fetcher.poll_next_unpin(&mut cx) {}
    }

    /// Runs dispatch and respond rounds until nothing is left to fetch. Returns the number of
    /// requests that were sent.
    fn run_to_completion(
        &mut self,
        respond: impl Fn(&[TxHash]) -> PooledTransactions<PooledTransactionVariant>,
    ) -> usize {
        let mut requests = 0;
        loop {
            let sent = self.dispatch();
            if sent == 0 {
                break
            }
            requests += sent;
            self.respond_all(&respond);
        }
        assert_eq!(self.fetcher.num_hashes(), 0, "all hashes must be fetched or given up on");
        requests
    }
}

fn peer_id(index: usize) -> PeerId {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(index as u64 + 1).to_be_bytes());
    PeerId::new(bytes)
}

fn pooled_txs(count: usize) -> Vec<PooledTransactionVariant> {
    let mut factory = MockTransactionFactory::default();
    (0..count)
        .map(|_| {
            let recovered: Recovered<TransactionSigned> =
                factory.create_eip1559().transaction.into();
            PooledTransactionVariant::try_from(recovered.into_inner()).unwrap()
        })
        .collect()
}

fn announcement_of(txs: &[PooledTransactionVariant]) -> Vec<(TxHash, Eth68TxMetadata)> {
    txs.iter().map(|tx| (*tx.tx_hash(), Some((TX_TYPE, TX_SIZE)))).collect()
}

/// Shared fixtures.
struct Fixtures {
    announcement: Vec<(TxHash, Eth68TxMetadata)>,
    by_hash: B256Map<PooledTransactionVariant>,
}

impl Fixtures {
    fn new() -> Self {
        let txs = pooled_txs(TXS);
        let announcement = announcement_of(&txs);
        let by_hash = txs.into_iter().map(|tx| (*tx.tx_hash(), tx)).collect();
        Self { announcement, by_hash }
    }

    /// Delivers every requested transaction.
    fn deliver(&self, hashes: &[TxHash]) -> PooledTransactions<PooledTransactionVariant> {
        PooledTransactions(hashes.iter().map(|hash| self.by_hash[hash].clone()).collect())
    }
}

fn bench_announce(c: &mut Criterion) {
    let fixtures = Fixtures::new();
    let mut group = c.benchmark_group("tx_fetcher/announce");

    group.throughput(Throughput::Elements((PEERS * TXS) as u64));
    group.bench_function("gossip", |b| {
        b.iter_batched_ref(
            || Rig::new(PEERS),
            |rig| rig.announce_gossip(&fixtures.announcement),
            BatchSize::LargeInput,
        )
    });

    group.throughput(Throughput::Elements(TXS as u64));
    group.bench_function("disjoint", |b| {
        b.iter_batched_ref(
            || Rig::new(PEERS),
            |rig| rig.announce_disjoint(&fixtures.announcement),
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

fn bench_dispatch(c: &mut Criterion) {
    let fixtures = Fixtures::new();
    let mut group = c.benchmark_group("tx_fetcher/dispatch");
    group.throughput(Throughput::Elements(TXS as u64));

    // sanity check the workload: a hash is only queued for the first peers that announced it, so
    // one dispatch sends a request of 256 hashes to each of them
    let mut rig = Rig::new(PEERS);
    rig.announce_gossip(&fixtures.announcement);
    assert_eq!(rig.dispatch(), MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH);

    group.bench_function("gossip", |b| {
        b.iter_batched_ref(
            || {
                let mut rig = Rig::new(PEERS);
                rig.announce_gossip(&fixtures.announcement);
                rig
            },
            |rig| rig.dispatch(),
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

fn bench_fetch(c: &mut Criterion) {
    let fixtures = Fixtures::new();
    let mut group = c.benchmark_group("tx_fetcher/fetch");
    group.throughput(Throughput::Elements(TXS as u64));

    // every request is answered in full: one request per 256 hashes
    let mut rig = Rig::new(PEERS);
    rig.announce_gossip(&fixtures.announcement);
    assert_eq!(rig.run_to_completion(|hashes| fixtures.deliver(hashes)), TXS / 256);

    group.bench_function("gossip_delivered", |b| {
        b.iter_batched_ref(
            || {
                let mut rig = Rig::new(PEERS);
                rig.announce_gossip(&fixtures.announcement);
                rig
            },
            |rig| rig.run_to_completion(|hashes| fixtures.deliver(hashes)),
            BatchSize::LargeInput,
        )
    });

    // every peer fails to deliver, so every hash is retried with all 8 candidates before it is
    // given up on
    let mut rig = Rig::new(8);
    rig.announce_gossip(&fixtures.announcement);
    assert_eq!(rig.run_to_completion(|_| PooledTransactions::default()), 8 * TXS / 256);

    group.bench_function("gossip_empty_responses", |b| {
        b.iter_batched_ref(
            || {
                let mut rig = Rig::new(8);
                rig.announce_gossip(&fixtures.announcement);
                rig
            },
            |rig| rig.run_to_completion(|_| PooledTransactions::default()),
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

criterion_group!(benches, bench_announce, bench_dispatch, bench_fetch);
criterion_main!(benches);
