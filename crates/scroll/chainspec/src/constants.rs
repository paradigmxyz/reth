use crate::genesis::L1Config;
use alloy_eips::eip1559::BaseFeeParams;
use alloy_primitives::{address, b256, Address, B256};

/// The transaction fee recipient on the L2.
pub const SCROLL_FEE_VAULT_ADDRESS: Address = address!("5300000000000000000000000000000000000005");

/// The system contract on L2 mainnet.
pub const SCROLL_MAINNET_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS: Address =
    address!("331A873a2a85219863d80d248F9e2978fE88D0Ea");

/// The L1 message queue address for Scroll mainnet.
/// <https://etherscan.io/address/0x0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B>.
pub const SCROLL_MAINNET_L1_MESSAGE_QUEUE_ADDRESS: Address =
    address!("0d7E906BD9cAFa154b048cFa766Cc1E54E39AF9B");

/// The L1 message queue v2 address for Scroll mainnet.
/// <https://etherscan.io/address/0x56971da63A3C0205184FEF096E9ddFc7A8C2D18a>.
pub const SCROLL_MAINNET_L1_MESSAGE_QUEUE_V2_ADDRESS: Address =
    address!("56971da63A3C0205184FEF096E9ddFc7A8C2D18a");

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
    l1_message_queue_v2_address: SCROLL_MAINNET_L1_MESSAGE_QUEUE_V2_ADDRESS,
    l2_system_config_address: SCROLL_MAINNET_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS,
    scroll_chain_address: SCROLL_MAINNET_L1_PROXY_ADDRESS,
    num_l1_messages_per_block: SCROLL_MAINNET_MAX_L1_MESSAGES,
};

/// The Scroll Mainnet genesis hash
pub const SCROLL_MAINNET_GENESIS_HASH: B256 =
    b256!("bbc05efd412b7cd47a2ed0e5ddfcf87af251e414ea4c801d78b6784513180a80");

/// The system contract on L2 sepolia.
pub const SCROLL_SEPOLIA_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS: Address =
    address!("F444cF06A3E3724e20B35c2989d3942ea8b59124");

/// The L1 message queue address for Scroll sepolia.
/// <https://sepolia.etherscan.io/address/0xF0B2293F5D834eAe920c6974D50957A1732de763>.
pub const SCROLL_SEPOLIA_L1_MESSAGE_QUEUE_ADDRESS: Address =
    address!("F0B2293F5D834eAe920c6974D50957A1732de763");

/// The L1 message queue address v2 for Scroll sepolia.
/// <https://sepolia.etherscan.io/address/0xA0673eC0A48aa924f067F1274EcD281A10c5f19F>.
pub const SCROLL_SEPOLIA_L1_MESSAGE_QUEUE_V2_ADDRESS: Address =
    address!("A0673eC0A48aa924f067F1274EcD281A10c5f19F");

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
    l1_message_queue_v2_address: SCROLL_SEPOLIA_L1_MESSAGE_QUEUE_V2_ADDRESS,
    l2_system_config_address: SCROLL_SEPOLIA_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS,
    scroll_chain_address: SCROLL_SEPOLIA_L1_PROXY_ADDRESS,
    num_l1_messages_per_block: SCROLL_SEPOLIA_MAX_L1_MESSAGES,
};

/// The system contract on devnet.
pub const SCROLL_DEV_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS: Address =
    address!("0000000000000000000000000000000000000000");

/// The L1 message queue address for Scroll dev.
pub const SCROLL_DEV_L1_MESSAGE_QUEUE_ADDRESS: Address =
    address!("0000000000000000000000000000000000000000");

/// The L1 message queue v2 address for Scroll dev.
pub const SCROLL_DEV_L1_MESSAGE_QUEUE_V2_ADDRESS: Address =
    address!("0000000000000000000000000000000000000000");

/// The L1 proxy address for Scroll dev.
pub const SCROLL_DEV_L1_PROXY_ADDRESS: Address =
    address!("0000000000000000000000000000000000000000");

/// The maximum allowed l1 messages per block for Scroll dev.
pub const SCROLL_DEV_MAX_L1_MESSAGES: u64 = 10;

/// The L1 configuration for Scroll dev.
pub const SCROLL_DEV_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: alloy_chains::NamedChain::Goerli as u64,
    l1_message_queue_address: SCROLL_DEV_L1_MESSAGE_QUEUE_ADDRESS,
    l1_message_queue_v2_address: SCROLL_DEV_L1_MESSAGE_QUEUE_V2_ADDRESS,
    scroll_chain_address: SCROLL_DEV_L1_PROXY_ADDRESS,
    l2_system_config_address: SCROLL_DEV_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS,
    num_l1_messages_per_block: SCROLL_DEV_MAX_L1_MESSAGES,
};

/// The Scroll Sepolia genesis hash
pub const SCROLL_SEPOLIA_GENESIS_HASH: B256 =
    b256!("aa62d1a8b2bffa9e5d2368b63aae0d98d54928bd713125e3fd9e5c896c68592c");

/// The base fee params for Feynman.
pub const SCROLL_BASE_FEE_PARAMS_FEYNMAN: BaseFeeParams = BaseFeeParams::new(
    SCROLL_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_FEYNMAN,
    SCROLL_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER_FEYNMAN,
);

/// The scroll EIP1559 max change denominator for Feynman.
pub const SCROLL_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_FEYNMAN: u128 = 8;

/// The scroll EIP1559 default elasticity multiplier for Feynman.
pub const SCROLL_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER_FEYNMAN: u128 = 2;
