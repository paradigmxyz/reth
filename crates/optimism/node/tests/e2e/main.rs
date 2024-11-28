#![allow(missing_docs)]
// Don't use the crate if `scroll` feature is used.
#![cfg_attr(feature = "scroll", allow(unused_crate_dependencies))]
#![cfg(not(feature = "scroll"))]

#[cfg(all(feature = "optimism", not(feature = "scroll")))]
mod p2p;

const fn main() {}
