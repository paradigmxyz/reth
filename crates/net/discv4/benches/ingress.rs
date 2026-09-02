//! Throughput of the discv4 receive path.
//!
//! `receive_loop` reads a datagram, decodes it, and forwards it to the service on one task, so
//! every packet's ECDSA recovery runs between two `recv_from` calls. This measures how many
//! packets that path can absorb.

#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use reth_discv4::{
    proto::{Message, Ping},
    IngressHandler,
};
use reth_network_peers::pk2id;
use secp256k1::{SecretKey, SECP256K1};
use std::{
    hint::black_box,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

/// Distinct valid packets from distinct keys, so every one costs a real recovery and none are
/// rejected as duplicates.
fn packets(count: usize) -> Vec<(Vec<u8>, SocketAddr)> {
    let mut rng = rand_08::thread_rng();
    (0..count)
        .map(|i| {
            let key = SecretKey::new(&mut rng);
            let endpoint = reth_discv4::proto::NodeEndpoint {
                address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                udp_port: 30303,
                tcp_port: 30303,
            };
            let msg = Message::Ping(Ping {
                from: endpoint,
                to: endpoint,
                expire: u64::MAX,
                enr_sq: Some(i as u64),
            });
            let (encoded, _) = msg.encode(&key);
            // a distinct source per packet, the handler rate limits per ip
            let src = SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, (i >> 16) as u8, (i >> 8) as u8, i as u8)),
                30303,
            );
            (encoded.to_vec(), src)
        })
        .collect()
}

fn ingress(c: &mut Criterion) {
    const BATCH: usize = 256;

    let rt =
        tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();

    let local_id = pk2id(&SecretKey::new(&mut rand_08::thread_rng()).public_key(SECP256K1));
    let batch = packets(BATCH);

    let mut group = c.benchmark_group("discv4_ingress");
    group.throughput(Throughput::Elements(BATCH as u64));

    // `readers` mirrors `Discv4Config::udp_ingress_readers`: how many tasks pull datagrams off
    // the socket and decode them against one shared handler.
    for readers in [1usize, 2, 4] {
        group.bench_function(format!("readers/{readers}"), |b| {
            b.iter_batched(
                || batch.clone(),
                |batch| {
                    rt.block_on(async {
                        let (handler, mut drain) = IngressHandler::in_memory(local_id, BATCH * 2);
                        let handler = Arc::new(handler);
                        let batch = Arc::new(batch);

                        let mut set = tokio::task::JoinSet::new();
                        for reader in 0..readers {
                            let (handler, batch) = (handler.clone(), batch.clone());
                            set.spawn(async move {
                                // each reader takes the datagrams the kernel would have handed it
                                for (data, src) in batch.iter().skip(reader).step_by(readers) {
                                    handler.handle_packet(black_box(data), *src).await;
                                }
                            });
                        }
                        while set.join_next().await.is_some() {}

                        let mut seen = drain();
                        while seen < BATCH {
                            tokio::task::yield_now().await;
                            seen += drain();
                        }
                        assert_eq!(seen, BATCH, "packets were dropped");
                        black_box(seen)
                    })
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

criterion_group!(benches, ingress);
criterion_main!(benches);
