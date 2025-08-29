//! Log index implementation for efficient log indexing.
//!
//! This crate provides the core implementation of log index,
//! which enables efficient querying of Ethereum logs without relying on bloom filters.
//!
//! ## Overview
//!
//! Log index is a two-dimensional sparse bit maps that index log values (addresses and topics)
//! for efficient searching. It provides:
//!
//! - Constant-time lookups for log queries
//! - Better storage efficiency compared to bloom filters
//! - Support for range queries across blocks
//! - Multi-layer overflow handling for collision management
//!
//! ## Usage
//!
//! ```rust
//! use reth_log_index::{
//!     FilterMapParams, LogValueIterator,
//!     query::query_logs_in_block_range,
//! };
//! use alloy_primitives::B256;
//! use alloy_rpc_types_eth::Filter;
//!
//! // Create filter map parameters
//! let params = FilterMapParams::default();
//!
//! // Create a log value iterator from a collection of log values
//! let log_values = vec![
//!     B256::from([1; 32]), // address hash
//!     B256::from([2; 32]), // topic hash
//! ];
//! let mut iterator = LogValueIterator::new(log_values.into_iter(), 0, params);
//!
//! // Process log values
//! while let Some(result) = iterator.next() {
//!     match result {
//!         Ok(cell) => println!("Processed log value at index {}", cell.index),
//!         Err(e) => println!("Error processing log value: {:?}", e),
//!     }
//! }
//!
//! // Query logs using filter maps (requires a FilterMapsReader implementation)
//! // let filter = Filter::new().address(address).topic0(topic);
//! // let results = query_logs_in_block_range(provider, &params, &filter, from_block, to_block)?;
//! ```

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod constants;
pub mod log_values;
mod params;
mod provider;
pub mod query;
mod types;
pub mod utils;

pub use constants::{DEFAULT_PARAMS, EXPECTED_MATCHES, MAX_LAYERS, RANGE_TEST_PARAMS};
pub use log_values::LogValueIterator;
pub use params::FilterMapParams;
pub use provider::{FilterMapsReader, FilterMapsWriter, MapLastBlockNumHash, MapValueRows};
pub use types::*;