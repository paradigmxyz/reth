//! Abstractions and runners for EF tests.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod case;
pub mod result;
pub mod suite;

pub mod assert;
pub mod cases;
pub mod models;

pub use case::{Case, Cases};
pub use result::{CaseResult, Error};
pub use suite::Suite;
