//!
//! `EthStats` client support for Reth.
//!
//! This crate provides the necessary components to connect to, authenticate with, and report
//! node and network statistics to an `EthStats` server. It includes abstractions for `WebSocket`
//! connections, error handling, event/message types, and the main `EthStats` service logic.
//!
//! - `connection`: `WebSocket` connection management and utilities
//! - `error`: Error types for connection and `EthStats` operations
//! - `ethstats`: Main service logic for `EthStats` client
//! - `events`: Data structures for `EthStats` protocol messages

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod connection;
mod credentials;

mod error;

mod ethstats;
pub use ethstats::*;

mod events;
pub use events::*;
