//! `trace_filter` types and support
use reth_primitives::{serde_helper::num::u64_hex_or_decimal_opt, Address};
use serde::{Deserialize, Serialize};

/// Trace filter.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct TraceFilter {
    /// From block
    #[serde(with = "u64_hex_or_decimal_opt")]
    pub from_block: Option<u64>,
    /// To block
    #[serde(with = "u64_hex_or_decimal_opt")]
    pub to_block: Option<u64>,
    /// From address
    pub from_address: Option<Vec<Address>>,
    /// To address
    pub to_address: Option<Vec<Address>>,
    /// Output offset
    pub after: Option<u64>,
    /// Output amount
    pub count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filter() {
        let s = r#"{"fromBlock":  "0x3","toBlock":  "0x5"}"#;
        let filter: TraceFilter = serde_json::from_str(s).unwrap();
        assert_eq!(filter.from_block, Some(3));
        assert_eq!(filter.to_block, Some(5));
    }
}
