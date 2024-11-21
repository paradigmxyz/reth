//! Pruning implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod builder;
mod db_ext;
mod error;
mod metrics;
mod pruner;
pub mod segments;

use crate::metrics::Metrics;
pub use builder::PrunerBuilder;
pub use error::PrunerError;
pub use pruner::{Pruner, PrunerResult, PrunerWithFactory, PrunerWithResult};

// Re-export prune types
#[doc(inline)]
pub use reth_prune_types::*;
