//! OP-Reth RPC support.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod error;
pub mod eth;
pub mod sequencer;

pub use error::{OpEthApiError, OptimismInvalidTransactionError, SequencerClientError};
pub use eth::{transaction::OpTxBuilder, OpEthApi, OpReceiptBuilder};
pub use sequencer::SequencerClient;
