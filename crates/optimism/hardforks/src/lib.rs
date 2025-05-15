//! OP-Reth hard forks.
//!
//! This defines the [`ChainHardforks`] for certain op chains.
//! It keeps L2 hardforks that correspond to L1 hardforks in sync by defining both at the same
//! activation timestamp, this includes:
//!  - Canyon : Shanghai
//!  - Ecotone : Cancun
//!  - Isthmus : Prague

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

extern crate alloc;

// Re-export alloy-op-hardforks types.
pub use alloy_op_hardforks::{base_mainnet::*, OpHardfork, OpHardforks};
pub use reth_ethereum_forks::ForkCondition;

use alloc::vec;
use alloy_primitives::U256;
use once_cell::sync::Lazy as LazyLock;
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, Hardfork};

/// Dev hardforks
pub static DEV_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Dao.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::London.boxed(), ForkCondition::ZERO_BLOCK),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 0,
                fork_block: None,
                total_difficulty: U256::ZERO,
            },
        ),
        (OpHardfork::Bedrock.boxed(), ForkCondition::ZERO_BLOCK),
        (OpHardfork::Regolith.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (OpHardfork::Canyon.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (OpHardfork::Ecotone.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (OpHardfork::Fjord.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (OpHardfork::Granite.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (EthereumHardfork::Prague.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (OpHardfork::Isthmus.boxed(), ForkCondition::ZERO_TIMESTAMP),
    ])
});

/// Optimism mainnet list of hardforks.
pub static OP_MAINNET_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(3950000)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(105235063)),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(105235063)),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(105235063)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 105235063,
                fork_block: Some(105235063),
                total_difficulty: U256::ZERO,
            },
        ),
        (OpHardfork::Bedrock.boxed(), ForkCondition::Block(105235063)),
        (OpHardfork::Regolith.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
        (OpHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
        (OpHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
        (OpHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
        (OpHardfork::Granite.boxed(), ForkCondition::Timestamp(1726070401)),
        (OpHardfork::Holocene.boxed(), ForkCondition::Timestamp(1736445601)),
        (EthereumHardfork::Prague.boxed(), ForkCondition::Timestamp(1746806401)),
        (OpHardfork::Isthmus.boxed(), ForkCondition::Timestamp(1746806401)),
    ])
});
/// Optimism Sepolia list of hardforks.
pub static OP_SEPOLIA_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::London.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 0,
                fork_block: Some(0),
                total_difficulty: U256::ZERO,
            },
        ),
        (OpHardfork::Bedrock.boxed(), ForkCondition::ZERO_BLOCK),
        (OpHardfork::Regolith.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
        (OpHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
        (OpHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
        (OpHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
        (OpHardfork::Granite.boxed(), ForkCondition::Timestamp(1723478400)),
        (OpHardfork::Holocene.boxed(), ForkCondition::Timestamp(1732633200)),
        (EthereumHardfork::Prague.boxed(), ForkCondition::Timestamp(1744905600)),
        (OpHardfork::Isthmus.boxed(), ForkCondition::Timestamp(1744905600)),
    ])
});

/// Base Sepolia list of hardforks.
pub static BASE_SEPOLIA_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::London.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 0,
                fork_block: Some(0),
                total_difficulty: U256::ZERO,
            },
        ),
        (OpHardfork::Bedrock.boxed(), ForkCondition::ZERO_BLOCK),
        (OpHardfork::Regolith.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
        (OpHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
        (OpHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
        (OpHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
        (OpHardfork::Granite.boxed(), ForkCondition::Timestamp(1723478400)),
        (OpHardfork::Holocene.boxed(), ForkCondition::Timestamp(1732633200)),
        (EthereumHardfork::Prague.boxed(), ForkCondition::Timestamp(1744905600)),
        (OpHardfork::Isthmus.boxed(), ForkCondition::Timestamp(1744905600)),
    ])
});

/// Base mainnet list of hardforks.
pub static BASE_MAINNET_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::London.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::ZERO_BLOCK),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 0,
                fork_block: Some(0),
                total_difficulty: U256::ZERO,
            },
        ),
        (OpHardfork::Bedrock.boxed(), ForkCondition::ZERO_BLOCK),
        (OpHardfork::Regolith.boxed(), ForkCondition::ZERO_TIMESTAMP),
        (
            EthereumHardfork::Shanghai.boxed(),
            ForkCondition::Timestamp(BASE_MAINNET_CANYON_TIMESTAMP),
        ),
        (OpHardfork::Canyon.boxed(), ForkCondition::Timestamp(BASE_MAINNET_CANYON_TIMESTAMP)),
        (
            EthereumHardfork::Cancun.boxed(),
            ForkCondition::Timestamp(BASE_MAINNET_ECOTONE_TIMESTAMP),
        ),
        (OpHardfork::Ecotone.boxed(), ForkCondition::Timestamp(BASE_MAINNET_ECOTONE_TIMESTAMP)),
        (OpHardfork::Fjord.boxed(), ForkCondition::Timestamp(BASE_MAINNET_FJORD_TIMESTAMP)),
        (OpHardfork::Granite.boxed(), ForkCondition::Timestamp(BASE_MAINNET_GRANITE_TIMESTAMP)),
        (OpHardfork::Holocene.boxed(), ForkCondition::Timestamp(BASE_MAINNET_HOLOCENE_TIMESTAMP)),
        (
            EthereumHardfork::Prague.boxed(),
            ForkCondition::Timestamp(BASE_MAINNET_ISTHMUS_TIMESTAMP),
        ),
        (OpHardfork::Isthmus.boxed(), ForkCondition::Timestamp(BASE_MAINNET_ISTHMUS_TIMESTAMP)),
    ])
});
