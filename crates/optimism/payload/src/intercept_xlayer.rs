//! OKB Bridge transaction interception for cross-chain protection
//!
//! This module intercepts bridge transactions targeting specific tokens (like OKB)
//! to prevent unauthorized cross-chain transfers.

use alloy_primitives::{Address, B256, U256};
use tracing::{debug, warn};

/// BridgeEvent signature hash
/// keccak256("BridgeEvent(uint8,uint32,address,uint32,address,uint256,bytes,uint32)")
pub(crate) const BRIDGE_EVENT_SIGNATURE: B256 = B256::new([
    0x50, 0x17, 0x81, 0x20, 0x9a, 0x1f, 0x88, 0x99, 0x32, 0x3b, 0x96, 0xb4, 0xef, 0x08, 0xb1, 0x68,
    0xdf, 0x93, 0xe0, 0xa9, 0x0c, 0x67, 0x3d, 0x1e, 0x4c, 0xce, 0x39, 0x36, 0x6c, 0xb6, 0x2f, 0x9b,
]);

/// Configuration for bridge transaction interception
///
/// This configuration determines which bridge transactions should be intercepted
/// and blocked during payload building. It supports both specific token filtering
/// and wildcard mode for blocking all bridge transactions.
#[derive(Debug, Default, Clone)]
pub struct BridgeInterceptConfig {
    /// Whether bridge interception is enabled
    pub enabled: bool,
    /// PolygonZkEVMBridge contract address to monitor
    pub bridge_contract_address: Address,
    /// Target token address to intercept (e.g., OKB token address)
    pub target_token_address: Address,
    /// Wildcard mode - intercept all bridge transactions from this contract
    pub wildcard: bool,
}

impl BridgeInterceptConfig {
    /// Create a new disabled config
    pub const fn new() -> Self {
        Self {
            enabled: false,
            bridge_contract_address: Address::ZERO,
            target_token_address: Address::ZERO,
            wildcard: false,
        }
    }
}

#[derive(Debug)]
struct BridgeEventData {
    /// Token address being bridged (originAddress in event)
    origin_address: Address,
    /// Amount being bridged
    amount: U256,
}

/// Errors that occur during bridge transaction interception
///
/// These errors indicate that a transaction has been intercepted and should be
/// excluded from the payload being built.
#[derive(Debug, thiserror::Error)]
pub enum BridgeInterceptError {
    /// Bridge transaction blocked in wildcard mode
    #[error("bridge transaction blocked (wildcard): bridge={bridge_contract}, sender={sender}")]
    WildcardBlock {
        /// Address of the bridge contract that emitted the event
        bridge_contract: Address,
        /// Address of the transaction sender
        sender: Address,
    },

    /// Bridge transaction blocked for target token
    #[error("bridge transaction blocked: token={token}, amount={amount}, sender={sender}")]
    TargetTokenBlock {
        /// Address of the token being bridged
        token: Address,
        /// Amount being bridged
        amount: U256,
        /// Address of the transaction sender
        sender: Address,
    },
}

/// Parse errors
#[derive(Debug)]
enum ParseError {
    InvalidSignature,
    InsufficientData,
}

/// Check if bridge transaction should be intercepted
///
/// Returns `Err` if transaction should be blocked, `Ok(())` if allowed
pub fn intercept_bridge_transaction_if_need(
    logs: &[alloy_primitives::Log],
    tx_sender: Address,
    config: &BridgeInterceptConfig,
) -> Result<(), BridgeInterceptError> {
    // Quick exit if feature disabled
    if !config.enabled {
        return Ok(());
    }

    // Check all logs
    for log in logs {
        // Only check logs from bridge contract
        if log.address != config.bridge_contract_address {
            continue;
        }

        // Wildcard mode: block all bridge transactions
        if config.wildcard {
            warn!(
                target: "payload_builder",
                bridge_contract = ?config.bridge_contract_address,
                tx_sender = ?tx_sender,
                "Bridge transaction intercepted (wildcard mode)"
            );
            return Err(BridgeInterceptError::WildcardBlock {
                bridge_contract: config.bridge_contract_address,
                sender: tx_sender,
            });
        }

        // Parse bridge event
        let event = match parser_bridge_event(log) {
            Ok(e) => e,
            Err(e) => {
                debug!(
                    target: "payload_builder",
                    error = ?e,
                    "Failed to parse bridge event, skipping"
                );
                continue;
            }
        };

        // Check if this is the target token
        if event.origin_address == config.target_token_address {
            warn!(
                target: "payload_builder",
                token = ?config.target_token_address,
                amount = ?event.amount,
                tx_sender = ?tx_sender,
                "Bridge transaction for target token intercepted"
            );
            return Err(BridgeInterceptError::TargetTokenBlock {
                token: config.target_token_address,
                amount: event.amount,
                sender: tx_sender,
            });
        }
    }

    Ok(())
}

/// Parse BridgeEvent from log data
fn parser_bridge_event(log: &alloy_primitives::Log) -> Result<BridgeEventData, ParseError> {
    // Check event signature
    if log.topics().first() != Some(&BRIDGE_EVENT_SIGNATURE) {
        return Err(ParseError::InvalidSignature);
    }

    let data = log.data.data.as_ref();
    if data.len() < 256 {
        return Err(ParseError::InsufficientData);
    }

    // Parse according to Solidity ABI encoding
    // event BridgeEvent(
    //     uint8 leafType,           // offset 0-31
    //     uint32 originNetwork,     // offset 32-63
    //     address originAddress,    // offset 64-95  ← We need this
    //     uint32 destinationNetwork,// offset 96-127
    //     address destinationAddress,// offset 128-159
    //     uint256 amount,           // offset 160-191 ← We need this
    //     bytes metadata,           // offset 192-223 (dynamic)
    //     uint32 depositCount       // offset 224-255
    // )

    // Extract origin address (bytes 76-96, address is 20 bytes, right-aligned)
    let origin_address = Address::from_slice(&data[76..96]);

    // Extract amount (bytes 160-192, uint256 is 32 bytes)
    let amount = U256::from_be_slice(&data[160..192]);

    Ok(BridgeEventData { origin_address, amount })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, Bytes, Log as AlloyLog, LogData};

    /// Helper function: create valid bridge event data with specified origin address
    fn create_valid_bridge_event_data(origin_address: Address) -> Bytes {
        let mut data = vec![0u8; 256];

        // Set origin address at offset 76-96 (right-aligned in 32-byte slot at offset 64-95)
        data[76..96].copy_from_slice(origin_address.as_slice());

        // Set a valid amount at offset 160-192 (1 ETH = 10^18 wei)
        let amount_bytes = U256::from(1_000_000_000_000_000_000u64).to_be_bytes::<32>();
        data[160..192].copy_from_slice(&amount_bytes);

        Bytes::from(data)
    }

    /// Helper function: create an Alloy log
    fn create_log(address: Address, topics: Vec<B256>, data: Bytes) -> AlloyLog {
        AlloyLog { address, data: LogData::new_unchecked(topics, data) }
    }

    #[test]
    fn test_parse_bridge_event_real_mainnet_data_first() {
        // Test case 1: First real bridge event from mainnet
        let data_hex = "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000075231f58b43240c9718dd58b4967c5114342a86c0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000bf7624b8a72797fe35ba1505587fc8a39705740c000000000000000000000000000000000000000000000000008e1bc9bf04000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000001c9700000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000034f4b42000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000034f4b420000000000000000000000000000000000000000000000000000000000";
        let data = Bytes::from(hex::decode(data_hex).unwrap());

        let log = create_log(Address::ZERO, vec![BRIDGE_EVENT_SIGNATURE], data);

        let event = parser_bridge_event(&log).expect("Failed to parse bridge event");

        // Verify origin address
        let expected_origin =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        assert_eq!(event.origin_address, expected_origin, "Origin address mismatch");

        // Verify amount (40000000000000000 wei = 0.04 ETH)
        let expected_amount = U256::from(40_000_000_000_000_000u64);
        assert_eq!(event.amount, expected_amount, "Amount mismatch");
    }

    #[test]
    fn test_parse_bridge_event_real_mainnet_data_second() {
        // Test case 2: Second real bridge event from mainnet with 1 wei amount
        let data_hex = "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000075231f58b43240c9718dd58b4967c5114342a86c0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000151ed65c4451661313848d07a615dec1f0d4ad25000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000001caa00000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000034f4b42000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000034f4b420000000000000000000000000000000000000000000000000000000000";
        let data = Bytes::from(hex::decode(data_hex).unwrap());

        let log = create_log(Address::ZERO, vec![BRIDGE_EVENT_SIGNATURE], data);

        let event = parser_bridge_event(&log).expect("Failed to parse bridge event");

        let expected_origin =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        assert_eq!(event.origin_address, expected_origin);

        // Amount is 1 wei in this case
        assert_eq!(event.amount, U256::from(1u64));
    }

    #[test]
    fn test_parse_bridge_event_real_mainnet_data_third() {
        // Test case 3: Third real bridge event from mainnet with large amount (13.92 ETH)
        let data_hex = "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000075231f58b43240c9718dd58b4967c5114342a86c0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000696314d0f50d0dfb3f6c8de9f33d9e546b1dfbed000000000000000000000000000000000000000000000000c12dc63fa970000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000001c9000000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000034f4b42000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000034f4b420000000000000000000000000000000000000000000000000000000000";
        let data = Bytes::from(hex::decode(data_hex).unwrap());

        let log = create_log(Address::ZERO, vec![BRIDGE_EVENT_SIGNATURE], data);

        let event = parser_bridge_event(&log).expect("Failed to parse bridge event");

        let expected_origin =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        assert_eq!(event.origin_address, expected_origin);

        // Amount is 13.92 ETH
        let expected_amount = U256::from(13_920_000_000_000_000_000u128);
        assert_eq!(event.amount, expected_amount);
    }

    #[test]
    fn test_parse_bridge_event_invalid_signature() {
        let data = Bytes::from(vec![0u8; 256]);
        let log = create_log(
            Address::ZERO,
            vec![B256::ZERO], // Invalid signature
            data,
        );

        let result = parser_bridge_event(&log);
        assert!(result.is_err(), "Expected error for invalid signature");
        assert!(matches!(result.unwrap_err(), ParseError::InvalidSignature));
    }

    #[test]
    fn test_parse_bridge_event_insufficient_data() {
        let data = Bytes::from(vec![0u8; 100]); // Only 100 bytes, need 256
        let log = create_log(Address::ZERO, vec![BRIDGE_EVENT_SIGNATURE], data);

        let result = parser_bridge_event(&log);
        assert!(result.is_err(), "Expected error for insufficient data");
        assert!(matches!(result.unwrap_err(), ParseError::InsufficientData));
    }

    #[test]
    fn test_parse_bridge_event_empty_topics() {
        let data = Bytes::from(vec![0u8; 256]);
        let log = create_log(
            Address::ZERO,
            vec![], // Empty topics
            data,
        );

        let result = parser_bridge_event(&log);
        assert!(result.is_err(), "Expected error for empty topics");
        assert!(matches!(result.unwrap_err(), ParseError::InvalidSignature));
    }

    #[test]
    fn test_intercept_disabled_config() {
        let config = BridgeInterceptConfig {
            enabled: false,
            bridge_contract_address: Address::from_slice(&[0x11; 20]),
            target_token_address: Address::from_slice(&[0x22; 20]),
            wildcard: false,
        };

        let sender = Address::from_slice(&[0x99; 20]);
        let log = create_log(
            config.bridge_contract_address,
            vec![BRIDGE_EVENT_SIGNATURE],
            create_valid_bridge_event_data(config.target_token_address),
        );

        let result = intercept_bridge_transaction_if_need(&[log], sender, &config);
        assert!(result.is_ok(), "Disabled config should allow all transactions");
    }

    #[test]
    fn test_intercept_target_token() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: false,
        };

        let log = create_log(
            bridge_contract,
            vec![BRIDGE_EVENT_SIGNATURE],
            create_valid_bridge_event_data(target_token),
        );

        let result = intercept_bridge_transaction_if_need(&[log], sender, &config);
        assert!(result.is_err(), "Should intercept target token");
        assert!(matches!(result.unwrap_err(), BridgeInterceptError::TargetTokenBlock { .. }));
    }

    #[test]
    fn test_intercept_non_target_token() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let other_token = Address::from_slice(&[0x11; 20]);
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: false,
        };

        let log = create_log(
            bridge_contract,
            vec![BRIDGE_EVENT_SIGNATURE],
            create_valid_bridge_event_data(other_token),
        );

        let result = intercept_bridge_transaction_if_need(&[log], sender, &config);
        assert!(result.is_ok(), "Should allow non-target token");
    }

    #[test]
    fn test_intercept_wildcard_mode() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let other_token = Address::from_slice(&[0x11; 20]);
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: true,
        };

        // Test with any token - should be intercepted in wildcard mode
        let log = create_log(
            bridge_contract,
            vec![BRIDGE_EVENT_SIGNATURE],
            create_valid_bridge_event_data(other_token),
        );

        let result = intercept_bridge_transaction_if_need(&[log], sender, &config);
        assert!(result.is_err(), "Wildcard mode should intercept any bridge tx");
        assert!(matches!(result.unwrap_err(), BridgeInterceptError::WildcardBlock { .. }));
    }

    #[test]
    fn test_intercept_non_bridge_contract() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let other_contract = Address::from_slice(&[0x11; 20]);
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: false,
        };

        // Log from different contract
        let log = create_log(
            other_contract,
            vec![BRIDGE_EVENT_SIGNATURE],
            create_valid_bridge_event_data(target_token),
        );

        let result = intercept_bridge_transaction_if_need(&[log], sender, &config);
        assert!(result.is_ok(), "Should ignore logs from other contracts");
    }

    #[test]
    fn test_intercept_multiple_logs() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let other_token = Address::from_slice(&[0x11; 20]);
        let other_contract = Address::from_slice(&[0x22; 20]);
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: false,
        };

        let logs = vec![
            // Log from other contract (should be ignored)
            create_log(
                other_contract,
                vec![BRIDGE_EVENT_SIGNATURE],
                create_valid_bridge_event_data(target_token),
            ),
            // Log with other token (should pass)
            create_log(
                bridge_contract,
                vec![BRIDGE_EVENT_SIGNATURE],
                create_valid_bridge_event_data(other_token),
            ),
            // Log with target token (should be intercepted)
            create_log(
                bridge_contract,
                vec![BRIDGE_EVENT_SIGNATURE],
                create_valid_bridge_event_data(target_token),
            ),
        ];

        let result = intercept_bridge_transaction_if_need(&logs, sender, &config);
        assert!(result.is_err(), "Should intercept when target token found in multiple logs");
    }

    #[test]
    fn test_intercept_wildcard_ignores_other_contracts() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let other_contract = Address::from_slice(&[0x11; 20]);
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: true,
        };

        // Log from other contract - should be ignored even in wildcard mode
        let log = create_log(
            other_contract,
            vec![BRIDGE_EVENT_SIGNATURE],
            create_valid_bridge_event_data(target_token),
        );

        let result = intercept_bridge_transaction_if_need(&[log], sender, &config);
        assert!(result.is_ok(), "Wildcard should only apply to configured bridge contract");
    }

    #[test]
    fn test_intercept_invalid_event_in_logs() {
        let bridge_contract =
            Address::from_slice(&hex::decode("2a3DD3EB832aF982ec71669E178424b10Dca2EDe").unwrap());
        let target_token =
            Address::from_slice(&hex::decode("75231f58b43240c9718dd58b4967c5114342a86c").unwrap());
        let sender = Address::from_slice(&[0x99; 20]);

        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: bridge_contract,
            target_token_address: target_token,
            wildcard: false,
        };

        let logs = vec![
            // Invalid signature (should be skipped)
            create_log(bridge_contract, vec![B256::ZERO], Bytes::from(vec![0u8; 256])),
            // Insufficient data (should be skipped)
            create_log(bridge_contract, vec![BRIDGE_EVENT_SIGNATURE], Bytes::from(vec![0u8; 100])),
            // Valid event with target token (should be intercepted)
            create_log(
                bridge_contract,
                vec![BRIDGE_EVENT_SIGNATURE],
                create_valid_bridge_event_data(target_token),
            ),
        ];

        let result = intercept_bridge_transaction_if_need(&logs, sender, &config);
        assert!(result.is_err(), "Should intercept valid event even with invalid events before it");
    }

    #[test]
    fn test_bridge_event_signature_constant() {
        // Verify the signature constant is correct
        // keccak256("BridgeEvent(uint8,uint32,address,uint32,address,uint256,bytes,uint32)")
        let expected = B256::new([
            0x50, 0x17, 0x81, 0x20, 0x9a, 0x1f, 0x88, 0x99, 0x32, 0x3b, 0x96, 0xb4, 0xef, 0x08,
            0xb1, 0x68, 0xdf, 0x93, 0xe0, 0xa9, 0x0c, 0x67, 0x3d, 0x1e, 0x4c, 0xce, 0x39, 0x36,
            0x6c, 0xb6, 0x2f, 0x9b,
        ]);
        assert_eq!(BRIDGE_EVENT_SIGNATURE, expected, "Bridge event signature mismatch");
    }

    #[test]
    fn test_empty_logs() {
        let config = BridgeInterceptConfig {
            enabled: true,
            bridge_contract_address: Address::from_slice(&[0x11; 20]),
            target_token_address: Address::from_slice(&[0x22; 20]),
            wildcard: false,
        };

        let sender = Address::from_slice(&[0x99; 20]);
        let result = intercept_bridge_transaction_if_need(&[], sender, &config);
        assert!(result.is_ok(), "Empty logs should pass");
    }
}
