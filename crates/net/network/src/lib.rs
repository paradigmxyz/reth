#![warn(missing_docs)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO remove later
#![allow(dead_code)]

//! reth network management.

mod config;
mod listener;
mod manager;
mod network;
mod peers;
mod swarm;
mod session;

pub use manager::NetworkManager;
pub use network::NetworkHandle;
