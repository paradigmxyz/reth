//! reth-bench library
//!
//! Provides benchmarking utilities for Ethereum execution clients.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

// Used by main.rs only
use color_eyre as _;
use reth_cli_util as _;

pub(crate) mod authenticated_transport;
pub mod bench;
pub(crate) mod bench_mode;
pub mod valid_payload;
