//! Hard forks of scroll protocol.

use alloy_hardforks::{hardfork, ForkCondition};

hardfork!(
    /// The name of the Scroll hardfork
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    pub const fn scroll_mainnet() -> [(Self, ForkCondition); 5] {
        [
            (Self::Archimedes, ForkCondition::Block(0)),
            (Self::Bernoulli, ForkCondition::Block(5220340)),
            (Self::Curie, ForkCondition::Block(7096836)),
            (Self::Darwin, ForkCondition::Timestamp(1724227200)),
            (Self::DarwinV2, ForkCondition::Timestamp(1725264000)),
        ]
    }

    /// Scroll sepolia list of hardforks.
    pub const fn scroll_sepolia() -> [(Self, ForkCondition); 5] {
        [
            (Self::Archimedes, ForkCondition::Block(0)),
            (Self::Bernoulli, ForkCondition::Block(3747132)),
            (Self::Curie, ForkCondition::Block(4740239)),
            (Self::Darwin, ForkCondition::Timestamp(1723622400)),
            (Self::DarwinV2, ForkCondition::Timestamp(1724832000)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
