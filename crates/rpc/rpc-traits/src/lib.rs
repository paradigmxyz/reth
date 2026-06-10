//! RPC conversion traits for Ethereum types.
//!
//! This crate provides traits for converting between consensus-layer types and RPC response types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

mod signable;
pub use signable::{SignTxRequestError, SignableTxRequest};

mod header;
pub use header::FromConsensusHeader;

mod transaction;
pub use transaction::{FromConsensusTx, TryIntoSimTx, TxInfoMapper};
