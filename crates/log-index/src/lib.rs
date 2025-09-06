//! Log Index implementation based on EIP-7745.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod constants;
/// The indexer that holds state and helps with indexing.
pub mod indexer;
mod params;
mod provider;
/// Functions for querying in a block range.
pub mod query;
mod types;
/// Utility functions
pub mod utils;

pub use constants::{DEFAULT_PARAMS, EXPECTED_MATCHES, MAX_LAYERS, RANGE_TEST_PARAMS};
pub use params::FilterMapParams;
pub use provider::{LogIndexProvider, MapValueRows};
pub use types::*;
