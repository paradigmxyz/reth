//! `snap/2` (EIP-8189) client request routing.
//!
//! Requests are dispatched through [`SnapClient`], which routes them to a snap-capable session via
//! the [`SessionManager`](crate::session::SessionManager); the session sends them on its dedicated
//! [`EthSnapStream`](reth_eth_wire::EthSnapStream) and correlates the response by `request_id`.

mod client;
mod routing;

pub use client::{SnapClient, SnapPeerRequest};
pub(crate) use routing::SnapRouting;
