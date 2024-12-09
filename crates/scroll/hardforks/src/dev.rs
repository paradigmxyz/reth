use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition};

#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
#[cfg(feature = "std")]
use std::sync::LazyLock;

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
        (crate::ScrollHardfork::Archimedes.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(0)),
        (crate::ScrollHardfork::Bernoulli.boxed(), ForkCondition::Block(0)),
        (crate::ScrollHardfork::Curie.boxed(), ForkCondition::Block(0)),
        (crate::ScrollHardfork::Darwin.boxed(), ForkCondition::Timestamp(0)),
        (crate::ScrollHardfork::DarwinV2.boxed(), ForkCondition::Timestamp(0)),
    ])
});
