#![allow(missing_docs)]
// Don't use the crate if `scroll` feature is used.
#![cfg_attr(feature = "scroll", allow(unused_crate_dependencies))]
#![cfg(not(feature = "scroll"))]

mod blobs;
mod dev;
mod eth;
mod p2p;
mod rpc;
mod utils;

const fn main() {}
