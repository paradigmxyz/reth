//! Hard forks of scroll protocol.

use alloc::{format, string::String, vec};
use core::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};

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

#[cfg(test)]
mod tests {
    use super::*;

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
