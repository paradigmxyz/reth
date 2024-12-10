use crate::genesis::L1Config;
use alloy_primitives::{address, Address};

/// The transaction fee recipient on the L2.
pub const SCROLL_FEE_VAULT_ADDRESS: Address = address!("5300000000000000000000000000000000000005");

/// The L1 message queue address for Scroll mainnet.
/// <https://etherscan.io/address/0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B>.
pub const SCROLL_MAINNET_L1_MESSAGE_QUEUE_ADDRESS: Address =
    address!("0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B");

/// The L1 proxy address for Scroll mainnet.
/// <https://etherscan.io/address/0xa13BAF47339d63B743e7Da8741db5456DAc1E556>.
pub const SCROLL_MAINNET_L1_PROXY_ADDRESS: Address =
    address!("a13BAF47339d63B743e7Da8741db5456DAc1E556");

/// The maximum allowed l1 messages per block for Scroll mainnet.
pub const SCROLL_MAINNET_MAX_L1_MESSAGES: u64 = 10;

/// The L1 configuration for Scroll mainnet.
pub const SCROLL_MAINNET_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: alloy_chains::NamedChain::Mainnet as u64,
    l1_message_queue_address: SCROLL_MAINNET_L1_MESSAGE_QUEUE_ADDRESS,
    scroll_chain_address: SCROLL_MAINNET_L1_PROXY_ADDRESS,
    num_l1_messages_per_block: SCROLL_MAINNET_MAX_L1_MESSAGES,
};

/// The L1 message queue address for Scroll sepolia.
/// <https://sepolia.etherscan.io/address/0xF0B2293F5D834eAe920c6974D50957A1732de763>.
pub const SCROLL_SEPOLIA_L1_MESSAGE_QUEUE_ADDRESS: Address =
    address!("F0B2293F5D834eAe920c6974D50957A1732de763");

/// The L1 proxy address for Scroll sepolia.
/// <https://sepolia.etherscan.io/address/0x2D567EcE699Eabe5afCd141eDB7A4f2D0D6ce8a0>
pub const SCROLL_SEPOLIA_L1_PROXY_ADDRESS: Address =
    address!("2D567EcE699Eabe5afCd141eDB7A4f2D0D6ce8a0");

/// The maximum allowed l1 messages per block for Scroll sepolia.
pub const SCROLL_SEPOLIA_MAX_L1_MESSAGES: u64 = 10;

/// The L1 configuration for Scroll sepolia.
pub const SCROLL_SEPOLIA_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: alloy_chains::NamedChain::Sepolia as u64,
    l1_message_queue_address: SCROLL_SEPOLIA_L1_MESSAGE_QUEUE_ADDRESS,
    scroll_chain_address: SCROLL_SEPOLIA_L1_PROXY_ADDRESS,
    num_l1_messages_per_block: SCROLL_SEPOLIA_MAX_L1_MESSAGES,
};
