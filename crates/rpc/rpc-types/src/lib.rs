//! Reth RPC type definitions.
//!
//! Provides all relevant types for the various RPC endpoints, grouped by namespace.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod admin;
pub mod beacon;
mod eth;
mod mev;
mod net;
mod otterscan;
mod peer;
pub mod relay;
mod rpc;
pub mod serde_helpers;

pub use admin::*;
pub use eth::*;
pub use mev::*;
pub use net::*;
pub use otterscan::*;
pub use peer::*;
pub use rpc::*;
pub use serde_helpers::*;
