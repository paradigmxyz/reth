//! Benchmarks for the discv4 wire codec.
//!
//! The receive path decodes every inbound datagram before any of the service level checks run, so
//! the cost measured here is paid once per packet, including for packets that are later dropped.

#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, Criterion};
use reth_discv4::proto::{Message, Neighbours, NodeEndpoint, Ping};
use reth_network_peers::NodeRecord;
use secp256k1::{SecretKey, SECP256K1};
use std::{
    hint::black_box,
    net::{IpAddr, Ipv4Addr},
};

const fn endpoint(port: u16) -> NodeEndpoint {
    NodeEndpoint { address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), udp_port: port, tcp_port: port }
}

const fn record(port: u16) -> NodeRecord {
    NodeRecord {
        address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        udp_port: port,
        tcp_port: port,
        id: alloy_primitives::FixedBytes::<64>::repeat_byte(port as u8),
    }
}

const fn ping() -> Message {
    Message::Ping(Ping {
        from: endpoint(30303),
        to: endpoint(30304),
        expire: u64::MAX,
        enr_sq: Some(1),
    })
}

/// A full neighbours response, the largest message on the wire in normal operation.
fn neighbours(len: usize) -> Message {
    Message::Neighbours(Neighbours {
        nodes: (0..len).map(|i| record(i as u16)).collect(),
        expire: u64::MAX,
    })
}

fn codec(c: &mut Criterion) {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let _ = SECP256K1;

    let mut group = c.benchmark_group("discv4_codec");

    for (name, msg) in
        [("ping", ping()), ("neighbours_4", neighbours(4)), ("neighbours_12", neighbours(12))]
    {
        let (encoded, _) = msg.encode(&secret_key);

        group.bench_function(format!("encode/{name}"), |b| {
            b.iter(|| black_box(black_box(&msg).encode(black_box(&secret_key))))
        });

        group.bench_function(format!("decode/{name}"), |b| {
            b.iter(|| black_box(Message::decode(black_box(&encoded)).unwrap()))
        });
    }

    group.finish();
}

criterion_group!(benches, codec);
criterion_main!(benches);
