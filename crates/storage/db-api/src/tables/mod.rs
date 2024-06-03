//! Tables and data models.
//!
//! # Overview
//!
//! This module defines the tables in reth, as well as some table-related abstractions:
//!
//! - [`codecs`] integrates different codecs into [`Encode`](crate::abstraction::table::Encode) and
//!   [`Decode`](crate::abstraction::table::Decode)
//! - [`models`] defines the values written to tables
//!
//! # Database Tour
//!
//! TODO(onbjerg): Find appropriate format for this...

// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]

pub mod codecs;
pub mod models;

mod raw;
pub use raw::{RawDupSort, RawKey, RawTable, RawValue, TableRawRow};

pub(crate) mod utils;
// Alias types.
// TODO(onbjerg): why are these here -.-

/// List with transaction numbers.
pub type BlockNumberList = IntegerList;

/// Encoded stage id.
pub type StageId = String;
