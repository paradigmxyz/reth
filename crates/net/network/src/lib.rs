#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! <reth crate template>

mod connections;
mod discovery;
mod listener;
mod network;
mod peers;

/// Identifier for a peer.
pub use reth_primitives::H512 as PeerId;

pub use network::Network;
