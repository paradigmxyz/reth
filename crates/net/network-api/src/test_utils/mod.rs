//! API for integration testing network components.
#![allow(missing_docs)]
pub mod peers_manager;

pub use peers_manager::{PeerCommand, PeersHandle, PeersHandleProvider};
