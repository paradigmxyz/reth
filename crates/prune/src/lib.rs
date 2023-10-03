//! Pruning implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, unreachable_pub, rustdoc::all)] // TODO(danipopes): missing_docs
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod error;
mod event;
mod metrics;
mod pruner;
mod segments;

use crate::metrics::Metrics;
pub use error::PrunerError;
pub use event::PrunerEvent;
pub use pruner::{Pruner, PrunerResult, PrunerWithResult};
