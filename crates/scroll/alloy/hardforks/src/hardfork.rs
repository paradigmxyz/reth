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
        /// Euclid <https://docs.scroll.io/en/technology/overview/scroll-upgrades/euclid-upgrade/>
        Euclid,
        /// EuclidV2 <https://docs.scroll.io/en/technology/overview/scroll-upgrades/euclid-upgrade/>
        EuclidV2,
        /// Feynman <https://docs.scroll.io/en/technology/overview/scroll-upgrades/feynman-upgrade/>
        Feynman
    }
);

impl ScrollHardfork {
    /// Scroll mainnet list of hardforks.
    pub const fn scroll_mainnet() -> [(Self, ForkCondition); 8] {
        [
            (Self::Archimedes, ForkCondition::Block(0)),
            (Self::Bernoulli, ForkCondition::Block(5220340)),
            (Self::Curie, ForkCondition::Block(7096836)),
            (Self::Darwin, ForkCondition::Timestamp(1724227200)),
            (Self::DarwinV2, ForkCondition::Timestamp(1725264000)),
            (Self::Euclid, ForkCondition::Timestamp(1744815600)),
            (Self::EuclidV2, ForkCondition::Timestamp(1745305200)),
            // TODO: update
            (Self::Feynman, ForkCondition::Timestamp(u64::MAX)),
        ]
    }

    /// Scroll sepolia list of hardforks.
    pub const fn scroll_sepolia() -> [(Self, ForkCondition); 8] {
        [
            (Self::Archimedes, ForkCondition::Block(0)),
            (Self::Bernoulli, ForkCondition::Block(3747132)),
            (Self::Curie, ForkCondition::Block(4740239)),
            (Self::Darwin, ForkCondition::Timestamp(1723622400)),
            (Self::DarwinV2, ForkCondition::Timestamp(1724832000)),
            (Self::Euclid, ForkCondition::Timestamp(1741680000)),
            (Self::EuclidV2, ForkCondition::Timestamp(1741852800)),
            // TODO: update
            (Self::Feynman, ForkCondition::Timestamp(u64::MAX)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn check_scroll_hardfork_from_str() {
        let hardfork_str =
            ["BernOulLi", "CUrie", "DaRwIn", "DaRwInV2", "EUcliD", "eUClidv2", "FEYnmaN"];
        let expected_hardforks = [
            ScrollHardfork::Bernoulli,
            ScrollHardfork::Curie,
            ScrollHardfork::Darwin,
            ScrollHardfork::DarwinV2,
            ScrollHardfork::Euclid,
            ScrollHardfork::EuclidV2,
            ScrollHardfork::Feynman,
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
