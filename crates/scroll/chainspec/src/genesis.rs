//! Scroll types for genesis data.

use crate::{
    constants::{SCROLL_FEE_VAULT_ADDRESS, SCROLL_MAINNET_L1_CONFIG, SCROLL_SEPOLIA_L1_CONFIG},
    SCROLL_DEV_L1_CONFIG,
};
use alloy_primitives::Address;
use alloy_serde::OtherFields;
use serde::de::Error;

/// Container type for all Scroll-specific fields in a genesis file.
/// This struct represents the configuration details and metadata
/// that are specific to the Scroll blockchain, used during the chain's initialization.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollChainInfo {
    /// Information about hard forks specific to the Scroll chain.
    /// This optional field contains metadata about various hard fork
    /// configurations that are specific to the Scroll blockchain.
    pub hard_fork_info: Option<ScrollHardforkInfo>,
    /// Scroll chain-specific configuration details.
    /// Encapsulates special parameters and settings
    /// required for Scroll chain functionality, such as fee-related
    /// addresses and Layer 1 configuration.
    pub scroll_chain_config: ScrollChainConfig,
}

impl ScrollChainInfo {
    /// Extracts the Scroll specific fields from a genesis file. These fields are expected to be
    /// contained in the `genesis.config` under `extra_fields` property.
    pub fn extract_from(others: &OtherFields) -> Option<Self> {
        Self::try_from(others).ok()
    }
}

impl TryFrom<&OtherFields> for ScrollChainInfo {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        let hard_fork_info = ScrollHardforkInfo::try_from(others).ok();
        let scroll_chain_config = ScrollChainConfig::try_from(others)?;

        Ok(Self { hard_fork_info, scroll_chain_config })
    }
}

/// [`ScrollHardforkInfo`] specifies the block numbers and timestamps at which the Scroll hardforks
/// were activated.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollHardforkInfo {
    /// archimedes block number
    pub archimedes_block: Option<u64>,
    /// bernoulli block number
    pub bernoulli_block: Option<u64>,
    /// curie block number
    pub curie_block: Option<u64>,
    /// darwin hardfork timestamp
    pub darwin_time: Option<u64>,
    /// darwinV2 hardfork timestamp
    pub darwin_v2_time: Option<u64>,
    /// euclid hardfork timestamp
    pub euclid_time: Option<u64>,
    /// euclidV2 hardfork timestamp
    pub euclid_v2_time: Option<u64>,
    /// feynman hardfork timestamp
    pub feynman_time: Option<u64>,
}

impl ScrollHardforkInfo {
    /// Extract the Scroll-specific genesis info from a genesis file.
    pub fn extract_from(others: &OtherFields) -> Option<Self> {
        Self::try_from(others).ok()
    }
}

impl TryFrom<&OtherFields> for ScrollHardforkInfo {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        others.deserialize_as()
    }
}

/// The Scroll l1 config
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1Config {
    /// l1 chain id
    pub l1_chain_id: u64,
    /// The L1 contract address of the contract that handles the message queue targeting the Scroll
    /// rollup.
    pub l1_message_queue_address: Address,
    /// The L1 contract address of the proxy contract which is responsible for Scroll rollup
    /// settlement.
    pub scroll_chain_address: Address,
    /// The maximum number of L1 messages to be consumed per L2 rollup block.
    pub num_l1_messages_per_block: u64,
}

/// The configuration for the Scroll sequencer chain.
/// This struct holds the configuration details specific to the Scroll chain,
/// including fee-related addresses and L1 chain-specific settings.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollChainConfig {
    /// The address of the L2 transaction fee vault.
    /// This is an optional field that, when set, specifies where L2 transaction fees
    /// will be sent or stored.
    pub fee_vault_address: Option<Address>,
    /// The L1 configuration.
    /// This field encapsulates specific settings and parameters required for L1
    pub l1_config: L1Config,
}

impl ScrollChainConfig {
    /// Extracts the scroll special info by looking for the `scroll` key. It is intended to be
    /// parsed from a genesis file.
    pub fn extract_from(others: &OtherFields) -> Option<Self> {
        Self::try_from(others).ok()
    }

    /// Returns the [`ScrollChainConfig`] for Scroll Mainnet.
    pub const fn mainnet() -> Self {
        Self {
            fee_vault_address: Some(SCROLL_FEE_VAULT_ADDRESS),
            l1_config: SCROLL_MAINNET_L1_CONFIG,
        }
    }

    /// Returns the [`ScrollChainConfig`] for Scroll Sepolia.
    pub const fn sepolia() -> Self {
        Self {
            fee_vault_address: Some(SCROLL_FEE_VAULT_ADDRESS),
            l1_config: SCROLL_SEPOLIA_L1_CONFIG,
        }
    }

    /// Returns the [`ScrollChainConfig`] for Scroll dev.
    pub const fn dev() -> Self {
        Self { fee_vault_address: Some(SCROLL_FEE_VAULT_ADDRESS), l1_config: SCROLL_DEV_L1_CONFIG }
    }
}

impl TryFrom<&OtherFields> for ScrollChainConfig {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        if let Some(Ok(scroll_chain_config)) = others.get_deserialized::<Self>("scroll") {
            Ok(scroll_chain_config)
        } else {
            Err(serde_json::Error::missing_field("scroll"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn test_extract_scroll_genesis_info() {
        let genesis_info = r#"
        {
          "archimedesBlock": 0,
          "bernoulliBlock": 10,
          "curieBlock": 12,
          "darwinTime": 0,
          "euclidTime": 11,
          "feynmanTime": 100
        }
        "#;

        let others: OtherFields = serde_json::from_str(genesis_info).unwrap();
        let genesis_info = ScrollHardforkInfo::extract_from(&others).unwrap();

        assert_eq!(
            genesis_info,
            ScrollHardforkInfo {
                archimedes_block: Some(0),
                bernoulli_block: Some(10),
                curie_block: Some(12),
                darwin_time: Some(0),
                darwin_v2_time: None,
                euclid_time: Some(11),
                euclid_v2_time: None,
                feynman_time: Some(100),
            }
        );
    }

    #[test]
    fn test_extract_scroll_chain_info() {
        let chain_info_str = r#"
        {
          "archimedesBlock": 0,
          "bernoulliBlock": 10,
          "curieBlock": 12,
          "darwinTime": 0,
          "euclidTime": 11,
          "feynmanTime": 100,
          "scroll": {
            "feeVaultAddress": "0x5300000000000000000000000000000000000005",
            "l1Config": {
                "l1ChainId": 1,
                "l1MessageQueueAddress": "0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B",
                "scrollChainAddress": "0xa13BAF47339d63B743e7Da8741db5456DAc1E556",
                "numL1MessagesPerBlock": 10
            }
          }
        }
        "#;

        let others: OtherFields = serde_json::from_str(chain_info_str).unwrap();
        let chain_info = ScrollChainInfo::extract_from(&others).unwrap();

        let expected = ScrollChainInfo {
            hard_fork_info: Some(ScrollHardforkInfo {
                archimedes_block: Some(0),
                bernoulli_block: Some(10),
                curie_block: Some(12),
                darwin_time: Some(0),
                darwin_v2_time: None,
                euclid_time: Some(11),
                euclid_v2_time: None,
                feynman_time: Some(100),
            }),
            scroll_chain_config: ScrollChainConfig {
                fee_vault_address: Some(address!("5300000000000000000000000000000000000005")),
                l1_config: L1Config {
                    l1_chain_id: 1,
                    l1_message_queue_address: address!("0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B"),
                    scroll_chain_address: address!("a13BAF47339d63B743e7Da8741db5456DAc1E556"),
                    num_l1_messages_per_block: 10,
                },
            },
        };
        assert_eq!(chain_info, expected);
    }
}
