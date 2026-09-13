//! Mapping math for the `FilterMaps` local search index.
//!
//! A filter map is a grid of rows by columns that holds a fixed number of *log values* — a log
//! value being one searchable item contributed by a log, either its emitting address or one of its
//! topics. This crate is the arithmetic core of that structure: the pure, storage-free functions
//! that decide which row and column of which map a given log value occupies, plus the parameter
//! set those functions read.
//!
//! The math is a port of go-ethereum's `core/filtermaps` package. Behavioral equivalence with Geth
//! is the contract: the index is only interoperable with Geth-compatible tooling, and Geth is only
//! usable as a correctness oracle, if these functions agree bit for bit. The port is pinned by
//! golden vectors generated from Geth (see `tests/golden`).
//!
//! Nothing here touches storage: these are pure functions over a log value and a position, and the
//! index that persists their output is built on top of them.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod params;
mod value;

pub use params::{Params, ParamsError, DEFAULT_PARAMS, RANGE_TEST_PARAMS};
pub use value::{address_value, topic_value};
