use crate::{ChainHardforks, EthereumHardfork, EthereumHardforks, Hardfork, OptimismHardfork};
use alloy_chains::Chain;
use core::any::Any;

/// Extends [`crate::EthereumHardforks`] with optimism helper methods.
pub trait OptimismHardforks: EthereumHardforks {
    /// Convenience method to check if [`OptimismHardfork::Bedrock`] is active at a given block
    /// number.
    fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(OptimismHardfork::Bedrock).active_at_block(block_number)
    }
}

impl OptimismHardforks for ChainHardforks {}

/// Trait that implements fork activation information helpers for [`ChainHardforks`].
pub trait OptimismActivations {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    fn activation_block<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64>;

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    fn activation_timestamp<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64>;

    /// Retrieves the activation block for the specified hardfork on the Base Sepolia testnet.
    fn base_sepolia_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
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
                OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(0),
                OptimismHardfork::Canyon => Some(2106456),
                OptimismHardfork::Ecotone => Some(6383256),
                OptimismHardfork::Fjord => Some(10615056),
            },
        )
    }

    /// Retrieves the activation block for the specified hardfork on the Base mainnet.
    fn base_mainnet_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
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
                OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(0),
                OptimismHardfork::Canyon => Some(9101527),
                OptimismHardfork::Ecotone => Some(11188936),
                _ => None,
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base Sepolia testnet.
    fn base_sepolia_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
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
                OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(1695768288),
                OptimismHardfork::Canyon => Some(1699981200),
                OptimismHardfork::Ecotone => Some(1708534800),
                OptimismHardfork::Fjord => Some(1716998400),
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base mainnet.
    fn base_mainnet_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
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
                OptimismHardfork::Bedrock | OptimismHardfork::Regolith => Some(1686789347),
                OptimismHardfork::Canyon => Some(1704992401),
                OptimismHardfork::Ecotone => Some(1710374401),
                OptimismHardfork::Fjord => Some(1720627201),
            },
        )
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

impl OptimismActivations for ChainHardforks {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    fn activation_block<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        if chain == Chain::base_sepolia() {
            return Self::base_sepolia_activation_block(fork)
        }
        if chain == Chain::base_mainnet() {
            return Self::base_mainnet_activation_block(fork)
        }

        None
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    fn activation_timestamp<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        if chain == Chain::base_sepolia() {
            return Self::base_sepolia_activation_timestamp(fork)
        }
        if chain == Chain::base_mainnet() {
            return Self::base_mainnet_activation_timestamp(fork)
        }

        None
    }
}
