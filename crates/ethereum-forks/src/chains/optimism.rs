use crate::{
    hardfork::optimism::OptimismHardfork, ChainHardforks, EthereumHardfork, ForkCondition,
};
use alloy_primitives::U256;
use once_cell::sync::Lazy;

/// Optimism mainnet hardforks
pub static OP_MAINNET_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(3950000)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(105235063)),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(105235063)),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(105235063)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(105235063), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(105235063)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
    ])
});

/// Optimism Sepolia hardforks
pub static OP_SEPOLIA_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
    ])
});

/// Base Sepolia hardforks
pub static BASE_SEPOLIA_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
    ])
});

/// Base Mainnet hardforks
pub static BASE_MAINNET_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
    ])
});
