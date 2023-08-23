#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![allow(clippy::result_large_err)]
#![feature(assert_matches)]
//! Crate for non-core stages. New stages added MAY be used by reth.
pub mod address;

pub use address::*;
use reth_db::{table::Table, tables, TableMetadata, TableType};

// Sets up a new NonCoreTable enum containing only these tables (no core reth tables).
tables!(NonCoreTable, 1, [(AddressAppearances, TableType::Table)]);

