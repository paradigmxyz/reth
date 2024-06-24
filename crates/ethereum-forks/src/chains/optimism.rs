use core::any::Any;

use super::ethereum::EthereumHardforks;
use crate::{
    hardfork::optimism::OptimismHardfork,
    hardforks::{ChainHardforks, Hardforks},
    ForkCondition, EthereumHardfork, Hardfork,
};
use alloy_primitives::U256;
use once_cell::sync::Lazy;

/// Extends [`EthereumHardforksTrait`] with optimism helper methods.
trait OptimismHardforks: EthereumHardforks {
    /// Convenience method to check if [`Hardfork::Bedrock`] is active at a given block number.
    fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(OptimismHardfork::Bedrock).active_at_block(block_number)
    }

    fn resolve<H, HF, OHF>(
        &self,
        fork: H,
        hardfork_fn: HF,
        optimism_hardfork_fn: OHF,
    ) -> Option<u64>
    where
        H: Hardfork,
        HF: Fn(&EthereumHardfork) -> Option<u64>,
        OHF: Fn(&OptimismHardfork) -> Option<u64>,
    {
        let fork: &dyn Any = &fork;
        if let Some(fork) = fork.downcast_ref::<EthereumHardfork>() {
            return hardfork_fn(fork)
        }
        fork.downcast_ref::<OptimismHardfork>().and_then(|fork| optimism_hardfork_fn(fork))
    }

    /// Retrieves the activation block for the specified hardfork on the Base Sepolia testnet.
    fn base_sepolia_activation_block<H: Hardfork>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    EthereumHardfork::Frontier |
                    EthereumHardfork::Homestead |
                    EthereumHardfork::Dao |
                    EthereumHardfork::Tangerine |
                    EthereumHardfork::SpuriousDragon |
                    EthereumHardfork::Byzantium |
                    EthereumHardfork::Constantinople |
                    EthereumHardfork::Petersburg |
                    EthereumHardfork::Istanbul |
                    EthereumHardfork::MuirGlacier |
                    EthereumHardfork::Berlin |
                    EthereumHardfork::London |
                    EthereumHardfork::ArrowGlacier |
                    EthereumHardfork::GrayGlacier |
                    EthereumHardfork::Paris |
                    EthereumHardfork::Shanghai => Some(2106456),
                    EthereumHardfork::Cancun => Some(6383256),
                    _ => None,
                }
            },
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(0),
                    OptimismHardfork::Canyon => Some(2106456),
                    OptimismHardfork::Ecotone => Some(6383256),
                    OptimismHardfork::Fjord => Some(10615056),
                    _ => None,
                }
            },
        )
    }

    /// Retrieves the activation block for the specified hardfork on the Base mainnet.
    fn base_mainnet_activation_block<H: Hardfork>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    EthereumHardfork::Frontier |
                    EthereumHardfork::Homestead |
                    EthereumHardfork::Dao |
                    EthereumHardfork::Tangerine |
                    EthereumHardfork::SpuriousDragon |
                    EthereumHardfork::Byzantium |
                    EthereumHardfork::Constantinople |
                    EthereumHardfork::Petersburg |
                    EthereumHardfork::Istanbul |
                    EthereumHardfork::MuirGlacier |
                    EthereumHardfork::Berlin |
                    EthereumHardfork::London |
                    EthereumHardfork::ArrowGlacier |
                    EthereumHardfork::GrayGlacier |
                    EthereumHardfork::Paris |
                    EthereumHardfork::Shanghai => Some(9101527),
                    EthereumHardfork::Cancun => Some(11188936),
                    _ => None,
                }
            },
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(0),
                    OptimismHardfork::Canyon => Some(9101527),
                    OptimismHardfork::Ecotone => Some(11188936),
                    _ => None,
                }
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base Sepolia testnet.
    fn base_sepolia_activation_timestamp<H: Hardfork>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    EthereumHardfork::Frontier |
                    EthereumHardfork::Homestead |
                    EthereumHardfork::Dao |
                    EthereumHardfork::Tangerine |
                    EthereumHardfork::SpuriousDragon |
                    EthereumHardfork::Byzantium |
                    EthereumHardfork::Constantinople |
                    EthereumHardfork::Petersburg |
                    EthereumHardfork::Istanbul |
                    EthereumHardfork::MuirGlacier |
                    EthereumHardfork::Berlin |
                    EthereumHardfork::London |
                    EthereumHardfork::ArrowGlacier |
                    EthereumHardfork::GrayGlacier |
                    EthereumHardfork::Paris |
                    EthereumHardfork::Shanghai => Some(1699981200),
                    EthereumHardfork::Cancun => Some(1708534800),
                    _ => None,
                }
            },
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(1695768288),
                    OptimismHardfork::Canyon => Some(1699981200),
                    OptimismHardfork::Ecotone => Some(1708534800),
                    OptimismHardfork::Fjord => Some(1716998400),
                    _ => None,
                }
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base mainnet.
    fn base_mainnet_activation_timestamp<H: Hardfork>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    EthereumHardfork::Frontier |
                    EthereumHardfork::Homestead |
                    EthereumHardfork::Dao |
                    EthereumHardfork::Tangerine |
                    EthereumHardfork::SpuriousDragon |
                    EthereumHardfork::Byzantium |
                    EthereumHardfork::Constantinople |
                    EthereumHardfork::Petersburg |
                    EthereumHardfork::Istanbul |
                    EthereumHardfork::MuirGlacier |
                    EthereumHardfork::Berlin |
                    EthereumHardfork::London |
                    EthereumHardfork::ArrowGlacier |
                    EthereumHardfork::GrayGlacier |
                    EthereumHardfork::Paris |
                    EthereumHardfork::Shanghai => Some(1704992401),
                    EthereumHardfork::Cancun => Some(1710374401),
                    _ => None,
                }
            },
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(1686789347),
                    OptimismHardfork::Canyon => Some(1704992401),
                    OptimismHardfork::Ecotone => Some(1710374401),
                    OptimismHardfork::Fjord => Some(1720627201),
                    _ => None,
                }
            },
        )
    }
}

impl OptimismHardforks for ChainHardforks {}

/// Optimism mainnet hardforks
pub static OP_MAINNET_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks(vec![
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
    ChainHardforks(vec![
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
    ChainHardforks(vec![
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
    ChainHardforks(vec![
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
