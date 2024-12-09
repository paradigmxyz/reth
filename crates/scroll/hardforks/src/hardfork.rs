//! Hard forks of scroll protocol.

use alloc::{boxed::Box, format, string::String, vec};
use core::{
    any::Any,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use alloy_chains::{Chain, NamedChain};
use reth_ethereum_forks::{hardfork, ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

hardfork!(
    /// The name of the Scroll hardfork
    ScrollHardfork {
        /// Archimedes: scroll test hardfork.
        Archimedes,
        /// Bernoulli: <https://scroll.io/blog/blobs-are-here-scrolls-bernoulli-upgrade>.
        Bernoulli,
        /// Curie: <https://scroll.io/blog/compressing-the-gas-scrolls-curie-upgrade>.
        Curie,
        /// Darwin: <https://scroll.io/blog/proof-recursion-scrolls-darwin-upgrade>.
        Darwin,
        /// DarwinV2 <https://x.com/Scroll_ZKP/status/1830565514755584269>.
        DarwinV2,
    }
);

impl ScrollHardfork {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    pub fn activation_block<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        // TODO(scroll): migrate to Chain::scroll() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
        if chain == Chain::from_named(NamedChain::Scroll) {
            return Self::scroll_sepolia_activation_block(fork);
        }
        // TODO(scroll): migrate to Chain::scroll_sepolia() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
        if chain == Chain::from_named(NamedChain::ScrollSepolia) {
            return Self::scroll_mainnet_activation_block(fork);
        }

        None
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    pub fn activation_timestamp<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        // TODO(scroll): migrate to Chain::scroll_sepolia() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
        if chain == Chain::from_named(NamedChain::ScrollSepolia) {
            return Self::scroll_sepolia_activation_timestamp(fork);
        }
        // TODO(scroll): migrate to Chain::scroll() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
        if chain == Chain::from_named(NamedChain::Scroll) {
            return Self::scroll_mainnet_activation_timestamp(fork);
        }

        None
    }

    /// Retrieves the activation block for the specified hardfork on the Scroll Sepolia testnet.
    pub fn scroll_sepolia_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Homestead |
                EthereumHardfork::Tangerine |
                EthereumHardfork::SpuriousDragon |
                EthereumHardfork::Byzantium |
                EthereumHardfork::Constantinople |
                EthereumHardfork::Petersburg |
                EthereumHardfork::Istanbul |
                EthereumHardfork::Berlin |
                EthereumHardfork::London |
                EthereumHardfork::Shanghai => Some(0),
                _ => None,
            },
            |fork| match fork {
                Self::Archimedes => Some(0),
                Self::Bernoulli => Some(3747132),
                Self::Curie => Some(4740239),
                Self::Darwin => Some(6075509),
                Self::DarwinV2 => Some(6375501),
            },
        )
    }

    /// Retrieves the activation block for the specified hardfork on the Scroll mainnet.
    pub fn scroll_mainnet_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Homestead |
                EthereumHardfork::Tangerine |
                EthereumHardfork::SpuriousDragon |
                EthereumHardfork::Byzantium |
                EthereumHardfork::Constantinople |
                EthereumHardfork::Petersburg |
                EthereumHardfork::Istanbul |
                EthereumHardfork::Berlin |
                EthereumHardfork::London |
                EthereumHardfork::Shanghai => Some(0),
                _ => None,
            },
            |fork| match fork {
                Self::Archimedes => Some(0),
                Self::Bernoulli => Some(5220340),
                Self::Curie => Some(7096836),
                Self::Darwin => Some(8568134),
                Self::DarwinV2 => Some(8923772),
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Scroll Sepolia testnet.
    pub fn scroll_sepolia_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Homestead |
                EthereumHardfork::Tangerine |
                EthereumHardfork::SpuriousDragon |
                EthereumHardfork::Byzantium |
                EthereumHardfork::Constantinople |
                EthereumHardfork::Petersburg |
                EthereumHardfork::Istanbul |
                EthereumHardfork::Berlin |
                EthereumHardfork::London |
                EthereumHardfork::Shanghai => Some(0),
                _ => None,
            },
            |fork| match fork {
                Self::Archimedes => Some(0),
                Self::Bernoulli => Some(1713175866),
                Self::Curie => Some(1718616171),
                Self::Darwin => Some(1723622400),
                Self::DarwinV2 => Some(1724832000),
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Scroll mainnet.
    pub fn scroll_mainnet_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Homestead |
                EthereumHardfork::Tangerine |
                EthereumHardfork::SpuriousDragon |
                EthereumHardfork::Byzantium |
                EthereumHardfork::Constantinople |
                EthereumHardfork::Petersburg |
                EthereumHardfork::Istanbul |
                EthereumHardfork::Berlin |
                EthereumHardfork::London |
                EthereumHardfork::Shanghai => Some(0),
                _ => None,
            },
            |fork| match fork {
                Self::Archimedes => Some(0),
                Self::Bernoulli => Some(1714358352),
                Self::Curie => Some(1719994277),
                Self::Darwin => Some(1724227200),
                Self::DarwinV2 => Some(1725264000),
            },
        )
    }

    /// Scroll mainnet list of hardforks.
    pub fn scroll_mainnet() -> ChainHardforks {
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
            (Self::Archimedes.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Block(0)),
            (Self::Bernoulli.boxed(), ForkCondition::Block(5220340)),
            (Self::Curie.boxed(), ForkCondition::Block(7096836)),
            (Self::Darwin.boxed(), ForkCondition::Timestamp(1724227200)),
            (Self::DarwinV2.boxed(), ForkCondition::Timestamp(1725264000)),
        ])
    }

    /// Scroll sepolia list of hardforks.
    pub fn scroll_sepolia() -> ChainHardforks {
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
            (Self::Archimedes.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Block(0)),
            (Self::Bernoulli.boxed(), ForkCondition::Block(3747132)),
            (Self::Curie.boxed(), ForkCondition::Block(4740239)),
            (Self::Darwin.boxed(), ForkCondition::Timestamp(1723622400)),
            (Self::DarwinV2.boxed(), ForkCondition::Timestamp(1724832000)),
        ])
    }
}

/// Match helper method since it's not possible to match on `dyn Hardfork`
fn match_hardfork<H, HF, SHF>(fork: H, hardfork_fn: HF, scroll_hardfork_fn: SHF) -> Option<u64>
where
    H: Hardfork,
    HF: Fn(&EthereumHardfork) -> Option<u64>,
    SHF: Fn(&ScrollHardfork) -> Option<u64>,
{
    let fork: &dyn Any = &fork;
    if let Some(fork) = fork.downcast_ref::<EthereumHardfork>() {
        return hardfork_fn(fork);
    }
    fork.downcast_ref::<ScrollHardfork>().and_then(scroll_hardfork_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_hardfork() {
        assert_eq!(
            ScrollHardfork::scroll_mainnet_activation_block(ScrollHardfork::Bernoulli),
            Some(5220340)
        );
        assert_eq!(
            ScrollHardfork::scroll_mainnet_activation_block(ScrollHardfork::Curie),
            Some(7096836)
        );
    }

    #[test]
    fn check_scroll_hardfork_from_str() {
        let hardfork_str = ["BernOulLi", "CUrie", "DaRwIn", "DaRwInV2"];
        let expected_hardforks = [
            ScrollHardfork::Bernoulli,
            ScrollHardfork::Curie,
            ScrollHardfork::Darwin,
            ScrollHardfork::DarwinV2,
        ];

        let hardforks: Vec<ScrollHardfork> =
            hardfork_str.iter().map(|h| ScrollHardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(ScrollHardfork::from_str("not a hardfork").is_err());
    }
}
