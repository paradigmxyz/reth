//! RESS protocol for stateless Ethereum nodes.
//!
//! Enables stateless nodes to fetch execution witnesses, bytecode, and block data from
//! stateful peers for minimal on-disk state with full execution capability.
//!
//! ## Node Types
//!
//! - **Stateless**: Minimal state, requests data on-demand
//! - **Stateful**: Full Ethereum nodes providing state data
//!
//! Valid connections: Stateless ↔ Stateless ✅, Stateless ↔ Stateful ✅, Stateful ↔ Stateful ❌
//!
//! ## Messages
//!
//! - `NodeType (0x00)`: Handshake
//! - `GetHeaders/Headers (0x01/0x02)`: Block headers
//! - `GetBlockBodies/BlockBodies (0x03/0x04)`: Block bodies
//! - `GetBytecode/Bytecode (0x05/0x06)`: Contract bytecode
//! - `GetWitness/Witness (0x07/0x08)`: Execution witnesses
//!
//! ## Flow
//!
//! 1. Exchange `NodeType` for compatibility
//! 2. Download ancestor blocks via headers/bodies
//! 3. For new payloads: request witness → get missing bytecode → execute
//!
//! Protocol version: `ress/1`

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod types;
pub use types::*;

mod message;
pub use message::*;

mod provider;
pub use provider::*;

mod handlers;
pub use handlers::*;

mod connection;
pub use connection::{RessPeerRequest, RessProtocolConnection};

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
