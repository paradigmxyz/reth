//! Types for the eth wire protocol: <https://github.com/ethereum/devp2p/blob/master/caps/eth.md>

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![allow(clippy::needless_lifetimes)] // side effect of optimism fields
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod status;
pub use status::{Status, StatusBuilder};

pub mod version;
pub use version::EthVersion;

pub mod message;
pub use message::{EthMessage, EthMessageID, ProtocolMessage};

pub mod blocks;
pub use blocks::*;

pub mod broadcast;
pub use broadcast::*;

pub mod transactions;
pub use transactions::*;

pub mod state;
pub use state::*;

pub mod receipts;
pub use receipts::*;
