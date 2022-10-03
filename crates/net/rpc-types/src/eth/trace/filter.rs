//! `trace_filter` types and support
use reth_primitives::{Address, BlockNumber};
use serde::Deserialize;

/// Trace filter.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct TraceFilter {
    /// From block
    pub from_block: Option<BlockNumber>,
    /// To block
    pub to_block: Option<BlockNumber>,
    /// From address
    pub from_address: Option<Vec<Address>>,
    /// To address
    pub to_address: Option<Vec<Address>>,
    /// Output offset
    pub after: Option<usize>,
    /// Output amount
    pub count: Option<usize>,
}
