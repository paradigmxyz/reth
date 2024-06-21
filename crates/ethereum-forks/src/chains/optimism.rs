use core::any::Any;

use super::ethereum::EthereumHardforksTrait;
use crate::{
    generate_forks_type,
    hardfork::optimism::OptimismHardfork,
    hardforks::{HardforksBaseType, HardforksTrait},
    EthereumForks, ForkCondition, Hardfork, HardforkTrait,
};
use alloy_primitives::U256;
use once_cell::sync::Lazy;

/// Extends [`EthereumHardforksTrait`] with optimism helper methods.
trait OptimismHardforksTrait: EthereumHardforksTrait {
    /// Convenience method to check if [`Hardfork::Bedrock`] is active at a given block number.
    fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(OptimismHardfork::Bedrock).active_at_block(block_number)
    }
}

generate_forks_type!(
    /// Wrapper type over a list of Optimism forks.
    OptimismForks
);

impl EthereumHardforksTrait for OptimismForks {}
impl OptimismHardforksTrait for OptimismForks {}

impl OptimismForks {
    fn resolve<H, HF, OHF>(
        &self,
        fork: H,
        hardfork_fn: HF,
        optimism_hardfork_fn: OHF,
    ) -> Option<u64>
    where
        H: HardforkTrait,
        HF: Fn(&Hardfork) -> Option<u64>,
        OHF: Fn(&OptimismHardfork) -> Option<u64>,
    {
        let fork: &dyn Any = &fork;
        if let Some(fork) = fork.downcast_ref::<Hardfork>() {
            return hardfork_fn(fork)
        }
        fork.downcast_ref::<OptimismHardfork>().and_then(|fork| optimism_hardfork_fn(fork))
    }

    /// Retrieves the activation block for the specified hardfork on the Base Sepolia testnet.
    fn base_sepolia_activation_block<H: HardforkTrait>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    Hardfork::Frontier |
                    Hardfork::Homestead |
                    Hardfork::Dao |
                    Hardfork::Tangerine |
                    Hardfork::SpuriousDragon |
                    Hardfork::Byzantium |
                    Hardfork::Constantinople |
                    Hardfork::Petersburg |
                    Hardfork::Istanbul |
                    Hardfork::MuirGlacier |
                    Hardfork::Berlin |
                    Hardfork::London |
                    Hardfork::ArrowGlacier |
                    Hardfork::GrayGlacier |
                    Hardfork::Paris |
                    Hardfork::Shanghai => Some(2106456),
                    Hardfork::Cancun => Some(6383256),
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
    fn base_mainnet_activation_block<H: HardforkTrait>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    Hardfork::Frontier |
                    Hardfork::Homestead |
                    Hardfork::Dao |
                    Hardfork::Tangerine |
                    Hardfork::SpuriousDragon |
                    Hardfork::Byzantium |
                    Hardfork::Constantinople |
                    Hardfork::Petersburg |
                    Hardfork::Istanbul |
                    Hardfork::MuirGlacier |
                    Hardfork::Berlin |
                    Hardfork::London |
                    Hardfork::ArrowGlacier |
                    Hardfork::GrayGlacier |
                    Hardfork::Paris |
                    Hardfork::Shanghai => Some(9101527),
                    Hardfork::Cancun => Some(11188936),
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
    fn base_sepolia_activation_timestamp<H: HardforkTrait>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    Hardfork::Frontier |
                    Hardfork::Homestead |
                    Hardfork::Dao |
                    Hardfork::Tangerine |
                    Hardfork::SpuriousDragon |
                    Hardfork::Byzantium |
                    Hardfork::Constantinople |
                    Hardfork::Petersburg |
                    Hardfork::Istanbul |
                    Hardfork::MuirGlacier |
                    Hardfork::Berlin |
                    Hardfork::London |
                    Hardfork::ArrowGlacier |
                    Hardfork::GrayGlacier |
                    Hardfork::Paris |
                    Hardfork::Shanghai => Some(1699981200),
                    Hardfork::Cancun => Some(1708534800),
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
    fn base_mainnet_activation_timestamp<H: HardforkTrait>(&self, fork: H) -> Option<u64> {
        self.resolve(
            fork,
            |fork| {
                #[allow(unreachable_patterns)]
                match fork {
                    Hardfork::Frontier |
                    Hardfork::Homestead |
                    Hardfork::Dao |
                    Hardfork::Tangerine |
                    Hardfork::SpuriousDragon |
                    Hardfork::Byzantium |
                    Hardfork::Constantinople |
                    Hardfork::Petersburg |
                    Hardfork::Istanbul |
                    Hardfork::MuirGlacier |
                    Hardfork::Berlin |
                    Hardfork::London |
                    Hardfork::ArrowGlacier |
                    Hardfork::GrayGlacier |
                    Hardfork::Paris |
                    Hardfork::Shanghai => Some(1704992401),
                    Hardfork::Cancun => Some(1710374401),
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

/// Optimism mainnet hardforks
pub static OP_MAINNET_HARDFORKS: Lazy<OptimismForks> = Lazy::new(|| {
    OptimismForks(vec![
        (Hardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (Hardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (Hardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (Hardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (Hardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (Hardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (Hardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Berlin.boxed(), ForkCondition::Block(3950000)),
        (Hardfork::London.boxed(), ForkCondition::Block(105235063)),
        (Hardfork::ArrowGlacier.boxed(), ForkCondition::Block(105235063)),
        (Hardfork::GrayGlacier.boxed(), ForkCondition::Block(105235063)),
        (
            Hardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(105235063), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(105235063)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (Hardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
        (Hardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
    ])
});

/// Optimism Sepolia hardforks
pub static OP_SEPOLIA_HARDFORKS: Lazy<OptimismForks> = Lazy::new(|| {
    OptimismForks(vec![
        (Hardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (Hardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (Hardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (Hardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (Hardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (Hardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (Hardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (Hardfork::London.boxed(), ForkCondition::Block(0)),
        (Hardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
        (
            Hardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (Hardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
        (Hardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
    ])
});

/// Base Sepolia hardforks
pub static BASE_SEPOLIA_HARDFORKS: Lazy<OptimismForks> = Lazy::new(|| {
    OptimismForks(vec![
        (Hardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (Hardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (Hardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (Hardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (Hardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (Hardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (Hardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (Hardfork::London.boxed(), ForkCondition::Block(0)),
        (Hardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
        (
            Hardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (Hardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
        (Hardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
    ])
});

/// Base Mainnet hardforks
pub static BASE_MAINNET_HARDFORKS: Lazy<OptimismForks> = Lazy::new(|| {
    OptimismForks(vec![
        (Hardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (Hardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (Hardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (Hardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (Hardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (Hardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (Hardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (Hardfork::London.boxed(), ForkCondition::Block(0)),
        (Hardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
        (Hardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
        (
            Hardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (Hardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
        (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
        (Hardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
        (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
    ])
});
