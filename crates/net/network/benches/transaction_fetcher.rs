#![allow(missing_docs)]

use alloy_primitives::B256;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use reth_eth_wire::{NewPooledTransactionHashes, NewPooledTransactionHashes68};
use reth_network::transactions::test_harness::{
    TransactionFetchHarness, TransactionFetchRequestStats,
};
use reth_network_peers::PeerId;
use tokio::runtime::Runtime;

// The disjoint workload intentionally matches the manager's default 4,096 pending-import
// capacity, so both implementations complete the same work in one poll.
const PEER_COUNT: usize = 64;
const HASHES_PER_PEER: usize = 64;
const ANNOUNCEMENT_ENTRY_COUNT: usize = PEER_COUNT * HASHES_PER_PEER;
// Keep request packing controlled by hash count rather than the 2 MiB response-size soft limit.
const TRANSACTION_ENCODED_SIZE: usize = 512;
const EIP1559_TRANSACTION_TYPE: u8 = 2;

struct BenchCase {
    harness: TransactionFetchHarness,
    announcements: Vec<(PeerId, NewPooledTransactionHashes)>,
}

impl BenchCase {
    async fn new(overlapping: bool) -> Self {
        let peers = (0..PEER_COUNT).map(peer_id).collect::<Vec<_>>();
        let announcements = peers
            .iter()
            .enumerate()
            .map(|(peer_index, peer_id)| {
                let start = if overlapping { 0 } else { peer_index * HASHES_PER_PEER };
                let hashes = (start..start + HASHES_PER_PEER).map(tx_hash).collect::<Vec<_>>();
                (
                    *peer_id,
                    NewPooledTransactionHashes::Eth68(NewPooledTransactionHashes68 {
                        types: vec![EIP1559_TRANSACTION_TYPE; hashes.len()],
                        sizes: vec![TRANSACTION_ENCODED_SIZE; hashes.len()],
                        hashes,
                    }),
                )
            })
            .collect();
        let harness = TransactionFetchHarness::new(peers).await;
        Self { harness, announcements }
    }

    async fn run(&mut self) {
        for (peer_id, announcement) in self.announcements.drain(..) {
            self.harness.announce(peer_id, announcement);
        }
        self.harness.poll_once().await;
    }
}

const fn peer_id(index: usize) -> PeerId {
    PeerId::new([(index + 1) as u8; 64])
}

fn tx_hash(index: usize) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&(index as u64 + 1).to_be_bytes());
    B256::from(bytes)
}

fn bench_transaction_fetcher(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("transaction_fetcher");
    group.throughput(Throughput::Elements(ANNOUNCEMENT_ENTRY_COUNT as u64));

    for (name, overlapping) in [("disjoint_announcements", false), ("overlapping_gossip", true)] {
        let unique_hash_count =
            if overlapping { HASHES_PER_PEER } else { ANNOUNCEMENT_ENTRY_COUNT };
        let expected = TransactionFetchRequestStats {
            request_count: if overlapping { 1 } else { PEER_COUNT },
            requested_hash_count: unique_hash_count,
            unique_requested_hash_count: unique_hash_count,
        };
        let mut validation_case = runtime.block_on(BenchCase::new(overlapping));
        runtime.block_on(validation_case.run());
        assert_eq!(validation_case.harness.drain_request_stats(), expected);

        group.bench_function(name, |b| {
            b.iter_batched_ref(
                || runtime.block_on(BenchCase::new(overlapping)),
                |case| runtime.block_on(case.run()),
                BatchSize::LargeInput,
            )
        });
    }

    group.finish();
}

criterion_group!(benches, bench_transaction_fetcher);
criterion_main!(benches);
