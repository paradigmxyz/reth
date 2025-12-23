//! Log pagination with cursor support for `eth_getLogsWithCursor` RPC method.

use alloy_rpc_types_eth::Log;
use serde::{Deserialize, Serialize};

/// Response for `eth_getLogsWithCursor` RPC method
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsWithCursor {
    pub logs: Vec<Log>,
    pub cursor: Option<String>,
}

/// Cursor position for pagination
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPosition {
    pub block_number: u64,
    pub log_index: u64,
}

impl CursorPosition {
    pub const fn new(block_number: u64, log_index: u64) -> Self {
        Self { block_number, log_index }
    }

    pub fn encode(&self) -> String {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&self.block_number.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.log_index.to_le_bytes());
        alloy_primitives::hex::encode_prefixed(bytes)
    }

    pub fn decode(cursor: &str) -> Option<Self> {
        let bytes = alloy_primitives::hex::decode(cursor).ok()?;
        if bytes.len() != 16 {
            return None;
        }
        let block_number = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let log_index = u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        Some(Self { block_number, log_index })
    }

    pub const fn is_before(&self, other: &Self) -> bool {
        self.block_number < other.block_number ||
            (self.block_number == other.block_number && self.log_index < other.log_index)
    }
}

impl LogsWithCursor {
    pub fn new(logs: Vec<Log>, cursor: Option<String>) -> Self {
        Self { logs, cursor }
    }

    pub fn final_page(logs: Vec<Log>) -> Self {
        Self { logs, cursor: None }
    }

    pub const fn empty() -> Self {
        Self { logs: Vec::new(), cursor: None }
    }

    pub fn with_pagination(logs: Vec<Log>, has_more: bool) -> Self {
        let cursor = (has_more && !logs.is_empty()).then(|| {
            let last_log = logs.last().unwrap();
            let position = CursorPosition::new(
                last_log.block_number.unwrap_or(0),
                last_log.log_index.unwrap_or(0),
            );
            position.encode()
        });
        Self { logs, cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logs_with_cursor_creation() {
        let logs = vec![];
        let response = LogsWithCursor::new(logs.clone(), Some("cursor".to_string()));
        assert_eq!(response.logs, logs);
        assert_eq!(response.cursor, Some("cursor".to_string()));
    }

    #[test]
    fn test_final_page() {
        let logs = vec![];
        let response = LogsWithCursor::final_page(logs.clone());
        assert_eq!(response.logs, logs);
        assert_eq!(response.cursor, None);
    }

    #[test]
    fn test_empty_response() {
        let response = LogsWithCursor::empty();
        assert!(response.logs.is_empty());
        assert_eq!(response.cursor, None);
    }

    #[test]
    fn test_cursor_position_encode_decode() {
        let position = CursorPosition::new(12345, 67890);
        let encoded = position.encode();
        let decoded = CursorPosition::decode(&encoded).expect("failed to decode");
        assert_eq!(decoded.block_number, 12345);
        assert_eq!(decoded.log_index, 67890);
    }

    #[test]
    fn test_cursor_position_roundtrip_max_values() {
        let position = CursorPosition::new(u64::MAX, u64::MAX);
        let encoded = position.encode();
        let decoded = CursorPosition::decode(&encoded).expect("failed to decode");
        assert_eq!(decoded, position);
    }

    #[test]
    fn test_cursor_position_roundtrip_zero() {
        let position = CursorPosition::new(0, 0);
        let encoded = position.encode();
        let decoded = CursorPosition::decode(&encoded).expect("failed to decode");
        assert_eq!(decoded, position);
    }

    #[test]
    fn test_cursor_position_is_before() {
        let pos1 = CursorPosition::new(100, 5);
        let pos2 = CursorPosition::new(100, 10);
        let pos3 = CursorPosition::new(101, 0);

        assert!(pos1.is_before(&pos2)); // same block, lower index
        assert!(pos1.is_before(&pos3)); // lower block
        assert!(!pos2.is_before(&pos1)); // higher index
        assert!(!pos3.is_before(&pos1)); // higher block
    }

    #[test]
    fn test_cursor_decode_invalid() {
        // Invalid hex
        assert!(CursorPosition::decode("invalid").is_none());
        // Wrong length
        assert!(CursorPosition::decode("0x1234").is_none());
        // Empty string
        assert!(CursorPosition::decode("").is_none());
    }
}
