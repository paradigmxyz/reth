//! Types for the eth wire protocol: <https://github.com/ethereum/devp2p/blob/master/caps/eth.md>

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod status;
pub use status::{Status, StatusBuilder, StatusEth69};

pub mod version;
pub use version::{EthVersion, ProtocolVersion};

pub mod message;
pub use message::{EthMessage, EthMessageID, ProtocolMessage};

pub mod header;
pub use header::*;

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

pub mod disconnect_reason;
pub use disconnect_reason::*;

pub mod capability;
pub use capability::*;

pub mod primitives;
pub use primitives::*;

mod snap;
pub use snap::*;

/// re-export for convenience
pub use alloy_eips::eip1898::{BlockHashOrNumber, HashOrNumber};
pub use alloy_eips::eip2718::Encodable2718;
