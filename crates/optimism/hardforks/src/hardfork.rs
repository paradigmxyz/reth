//! Hard forks of optimism protocol.

use alloc::{boxed::Box, format, string::String, vec};
use core::{
    any::Any,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use alloy_chains::Chain;
use alloy_primitives::U256;
use reth_ethereum_forks::{hardfork, ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

hardfork!(
    /// The name of an optimism hardfork.
    ///
    /// When building a list of hardforks for a chain, it's still expected to mix with
    /// [`EthereumHardfork`].
    OptimismHardfork {
        /// Bedrock: <https://blog.oplabs.co/introducing-optimism-bedrock>.
        Bedrock,
        /// Regolith: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#regolith>.
        Regolith,
        /// <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#canyon>.
        Canyon,
        /// Ecotone: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#ecotone>.
        Ecotone,
        /// Fjord: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#fjord>
        Fjord,
        /// Granite: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#granite>
        Granite,
    }
);

impl OptimismHardfork {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    pub fn activation_block<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        if chain == Chain::base_sepolia() {
            return Self::base_sepolia_activation_block(fork)
        }
        if chain == Chain::base_mainnet() {
            return Self::base_mainnet_activation_block(fork)
        }

        None
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    pub fn activation_timestamp<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        if chain == Chain::base_sepolia() {
            return Self::base_sepolia_activation_timestamp(fork)
        }
        if chain == Chain::base_mainnet() {
            return Self::base_mainnet_activation_timestamp(fork)
        }

        None
    }

    /// Retrieves the activation block for the specified hardfork on the Base Sepolia testnet.
    pub fn base_sepolia_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
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
            },
            |fork| match fork {
                Self::Bedrock | Self::Regolith => Some(0),
                Self::Canyon => Some(2106456),
                Self::Ecotone => Some(6383256),
                Self::Fjord => Some(10615056),
                _ => None,
            },
        )
    }

    /// Retrieves the activation block for the specified hardfork on the Base mainnet.
    pub fn base_mainnet_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
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
            },
            |fork| match fork {
                Self::Bedrock | Self::Regolith => Some(0),
                Self::Canyon => Some(9101527),
                Self::Ecotone => Some(11188936),
                _ => None,
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base Sepolia testnet.
    pub fn base_sepolia_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
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
            },
            |fork| match fork {
                Self::Bedrock | Self::Regolith => Some(1695768288),
                Self::Canyon => Some(1699981200),
                Self::Ecotone => Some(1708534800),
                Self::Fjord => Some(1716998400),
                Self::Granite => Some(1723478400),
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base mainnet.
    pub fn base_mainnet_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
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
            },
            |fork| match fork {
                Self::Bedrock | Self::Regolith => Some(1686789347),
                Self::Canyon => Some(1704992401),
                Self::Ecotone => Some(1710374401),
                Self::Fjord => Some(1720627201),
                Self::Granite => Some(1726070401),
            },
        )
    }

    /// Optimism mainnet list of hardforks.
    pub fn op_mainnet() -> ChainHardforks {
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
            (Self::Bedrock.boxed(), ForkCondition::Block(105235063)),
            (Self::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
            (Self::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
            (Self::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
            (Self::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
            (Self::Granite.boxed(), ForkCondition::Timestamp(1726070401)),
        ])
    }

    /// Optimism sepolia list of hardforks.
    pub fn op_sepolia() -> ChainHardforks {
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
            (Self::Bedrock.boxed(), ForkCondition::Block(0)),
            (Self::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
            (Self::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
            (Self::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
            (Self::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
            (Self::Granite.boxed(), ForkCondition::Timestamp(1723478400)),
        ])
    }

    /// Base sepolia list of hardforks.
    pub fn base_sepolia() -> ChainHardforks {
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
            (Self::Bedrock.boxed(), ForkCondition::Block(0)),
            (Self::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
            (Self::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
            (Self::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
            (Self::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
            (Self::Granite.boxed(), ForkCondition::Timestamp(1723478400)),
        ])
    }

    /// Base mainnet list of hardforks.
    pub fn base_mainnet() -> ChainHardforks {
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
            (Self::Bedrock.boxed(), ForkCondition::Block(0)),
            (Self::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
            (Self::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
            (Self::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
            (Self::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
            (Self::Granite.boxed(), ForkCondition::Timestamp(1726070401)),
        ])
    }
}

/// Match helper method since it's not possible to match on `dyn Hardfork`
fn match_hardfork<H, HF, OHF>(fork: H, hardfork_fn: HF, optimism_hardfork_fn: OHF) -> Option<u64>
where
    H: Hardfork,
    HF: Fn(&EthereumHardfork) -> Option<u64>,
    OHF: Fn(&OptimismHardfork) -> Option<u64>,
{
    let fork: &dyn Any = &fork;
    if let Some(fork) = fork.downcast_ref::<EthereumHardfork>() {
        return hardfork_fn(fork)
    }
    fork.downcast_ref::<OptimismHardfork>().and_then(optimism_hardfork_fn)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_match_hardfork() {
        assert_eq!(
            OptimismHardfork::base_mainnet_activation_block(EthereumHardfork::Cancun),
            Some(11188936)
        );
        assert_eq!(
            OptimismHardfork::base_mainnet_activation_block(OptimismHardfork::Canyon),
            Some(9101527)
        );
    }

    #[test]
    fn check_op_hardfork_from_str() {
        let hardfork_str = ["beDrOck", "rEgOlITH", "cAnYoN", "eCoToNe", "FJorD", "GRaNiTe"];
        let expected_hardforks = [
            OptimismHardfork::Bedrock,
            OptimismHardfork::Regolith,
            OptimismHardfork::Canyon,
            OptimismHardfork::Ecotone,
            OptimismHardfork::Fjord,
            OptimismHardfork::Granite,
        ];

        let hardforks: Vec<OptimismHardfork> =
            hardfork_str.iter().map(|h| OptimismHardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(OptimismHardfork::from_str("not a hardfork").is_err());
    }
}
