#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Internal helpers for network testing.

mod init;
mod testnet;

pub use init::{create_new_geth, enr_to_peer_id, unused_port, unused_tcp_udp, GETH_TIMEOUT};
pub use testnet::{NetworkEventStream, PeerConfig, Testnet};
