use crate::{ChainHardforks, EthereumHardfork, ForkCondition};
use alloc::vec;
use alloy_primitives::U256;
use once_cell::sync::Lazy;

/// Dev hardforks
pub static DEV_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Dao.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: None, total_difficulty: U256::ZERO },
        ),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(0)),
        #[cfg(feature = "optimism")]
        (crate::OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        #[cfg(feature = "optimism")]
        (crate::OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        #[cfg(feature = "optimism")]
        (crate::OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(0)),
        #[cfg(feature = "optimism")]
        (crate::OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(0)),
    ])
});
