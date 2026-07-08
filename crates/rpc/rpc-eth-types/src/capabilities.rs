//! Types for `eth_capabilities`.

use alloy_primitives::{B256, U64};
use serde::{Deserialize, Serialize};

/// Effective routing capabilities for the `eth` namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthCapabilities {
    /// Current chain head.
    pub head: EthCapabilitiesHead,
    /// Account and storage state availability.
    pub state: EthCapabilitiesResource,
    /// Transaction lookup availability.
    pub tx: EthCapabilitiesResource,
    /// Log query availability.
    pub logs: EthCapabilitiesResource,
    /// Receipt availability.
    pub receipts: EthCapabilitiesResource,
    /// Block header and body availability.
    pub blocks: EthCapabilitiesResource,
    /// State proof availability.
    #[serde(rename = "stateproofs")]
    pub state_proofs: EthCapabilitiesResource,
}

/// Current head block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthCapabilitiesHead {
    /// Head block number.
    #[serde(with = "alloy_serde::quantity")]
    pub number: u64,
    /// Head block hash.
    pub hash: B256,
}

/// Effective capability for one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthCapabilitiesResource {
    /// Whether this resource is unavailable.
    pub disabled: bool,
    /// Oldest block expected to be served correctly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_block: Option<U64>,
    /// Deletion strategy, if the resource is pruned by a sliding window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_strategy: Option<EthCapabilitiesDeleteStrategy>,
}

impl EthCapabilitiesResource {
    /// Creates an enabled resource available from the given oldest block.
    pub fn available_from(oldest_block: u64) -> Self {
        Self { disabled: false, oldest_block: Some(U64::from(oldest_block)), delete_strategy: None }
    }

    /// Creates an enabled resource with a sliding window deletion strategy.
    pub fn window(oldest_block: u64, retention_blocks: u64) -> Self {
        Self {
            disabled: false,
            oldest_block: Some(U64::from(oldest_block)),
            delete_strategy: Some(EthCapabilitiesDeleteStrategy {
                strategy_type: EthCapabilitiesDeleteStrategyKind::Window,
                retention_blocks,
            }),
        }
    }

    /// Creates a disabled resource.
    pub const fn disabled() -> Self {
        Self { disabled: true, oldest_block: None, delete_strategy: None }
    }
}

/// Deletion strategy for a resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthCapabilitiesDeleteStrategy {
    /// Strategy type.
    #[serde(rename = "type")]
    pub strategy_type: EthCapabilitiesDeleteStrategyKind,
    /// Number of blocks retained by the sliding window.
    #[serde(with = "alloy_serde::quantity")]
    pub retention_blocks: u64,
}

/// Deletion strategy kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EthCapabilitiesDeleteStrategyKind {
    /// Sliding window deletion.
    #[serde(rename = "window")]
    Window,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use serde_json::json;

    #[test]
    fn serializes_capabilities_schema_shape() {
        let capabilities = EthCapabilities {
            head: EthCapabilitiesHead { number: 1, hash: B256::ZERO },
            state: EthCapabilitiesResource::window(10, 90),
            tx: EthCapabilitiesResource::available_from(0),
            logs: EthCapabilitiesResource::available_from(0),
            receipts: EthCapabilitiesResource::available_from(0),
            blocks: EthCapabilitiesResource::available_from(0),
            state_proofs: EthCapabilitiesResource::disabled(),
        };

        let value = serde_json::to_value(capabilities).unwrap();

        assert_eq!(
            value,
            json!({
                "head": {
                    "number": "0x1",
                    "hash": B256::ZERO,
                },
                "state": {
                    "disabled": false,
                    "oldestBlock": "0xa",
                    "deleteStrategy": {
                        "type": "window",
                        "retentionBlocks": "0x5a",
                    },
                },
                "tx": {
                    "disabled": false,
                    "oldestBlock": "0x0",
                },
                "logs": {
                    "disabled": false,
                    "oldestBlock": "0x0",
                },
                "receipts": {
                    "disabled": false,
                    "oldestBlock": "0x0",
                },
                "blocks": {
                    "disabled": false,
                    "oldestBlock": "0x0",
                },
                "stateproofs": {
                    "disabled": true,
                },
            })
        );
    }
}
