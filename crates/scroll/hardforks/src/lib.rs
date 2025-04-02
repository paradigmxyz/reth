//! Scroll-Reth hard forks.

#![doc = include_str!("../docs/hardforks.md")]

use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};

// Re-export scroll-alloy-hardforks types.
pub use scroll_alloy_hardforks::{ScrollHardfork, ScrollHardforks};

#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
#[cfg(feature = "std")]
use std::sync::LazyLock;

/// Scroll mainnet hardforks
pub static SCROLL_MAINNET_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Dao.boxed(), ForkCondition::Never),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Never),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Never),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Never),
        (ScrollHardfork::Archimedes.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Block(0)),
        (ScrollHardfork::Bernoulli.boxed(), ForkCondition::Block(5220340)),
        (ScrollHardfork::Curie.boxed(), ForkCondition::Block(7096836)),
        (ScrollHardfork::Darwin.boxed(), ForkCondition::Timestamp(1724227200)),
        (ScrollHardfork::DarwinV2.boxed(), ForkCondition::Timestamp(1725264000)),
    ])
});

/// Scroll sepolia hardforks
pub static SCROLL_SEPOLIA_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (ScrollHardfork::Archimedes.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Block(0)),
        (ScrollHardfork::Bernoulli.boxed(), ForkCondition::Block(3747132)),
        (ScrollHardfork::Curie.boxed(), ForkCondition::Block(4740239)),
        (ScrollHardfork::Darwin.boxed(), ForkCondition::Timestamp(1723622400)),
        (ScrollHardfork::DarwinV2.boxed(), ForkCondition::Timestamp(1724832000)),
    ])
});

/// Dev hardforks
pub static DEV_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (ScrollHardfork::Archimedes.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(0)),
        (ScrollHardfork::Bernoulli.boxed(), ForkCondition::Block(0)),
        (ScrollHardfork::Curie.boxed(), ForkCondition::Block(0)),
        (ScrollHardfork::Darwin.boxed(), ForkCondition::Timestamp(0)),
        (ScrollHardfork::DarwinV2.boxed(), ForkCondition::Timestamp(0)),
    ])
});
