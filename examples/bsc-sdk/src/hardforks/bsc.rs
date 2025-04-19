#![allow(unused)]
use alloy_chains::Chain;
use core::any::Any;
use reth_chainspec::ForkCondition;
use reth_ethereum_forks::{hardfork, ChainHardforks, EthereumHardfork, Hardfork};

hardfork!(
    /// The name of a bsc hardfork.
    ///
    /// When building a list of hardforks for a chain, it's still expected to mix with [`EthereumHardfork`].
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    BscHardfork {
        /// BSC `Ramanujan` hardfork
        Ramanujan,
        /// BSC `Niels` hardfork
        Niels,
        /// BSC `MirrorSync` hardfork
        MirrorSync,
        /// BSC `Bruno` hardfork
        Bruno,
        /// BSC `Euler` hardfork
        Euler,
        /// BSC `Nano` hardfork
        Nano,
        /// BSC `Moran` hardfork
        Moran,
        /// BSC `Gibbs` hardfork
        Gibbs,
        /// BSC `Planck` hardfork
        Planck,
        /// BSC `Luban` hardfork
        Luban,
        /// BSC `Plato` hardfork
        Plato,
        /// BSC `Hertz` hardfork
        Hertz,
        /// BSC `HertzFix` hardfork
        HertzFix,
        /// BSC `Kepler` hardfork
        Kepler,
        /// BSC `Feynman` hardfork
        Feynman,
        /// BSC `FeynmanFix` hardfork
        FeynmanFix,
        /// BSC `Haber` hardfork
        Haber,
        /// BSC `HaberFix` hardfork
        HaberFix,
        /// BSC `Bohr` hardfork
        Bohr,
        /// BSC `Pascal` hardfork
        Pascal,
        /// BSC `Prague` hardfork
        Prague,
    }
);

impl BscHardfork {
    /// Retrieves the activation block for the specified hardfork on the given chain.
    pub fn activation_block<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        if chain == Chain::bsc_mainnet() {
            return Self::bsc_mainnet_activation_block(fork)
        }
        if chain == Chain::bsc_testnet() {
            return Self::bsc_testnet_activation_block(fork)
        }

        None
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    pub fn activation_timestamp<H: Hardfork>(self, fork: H, chain: Chain) -> Option<u64> {
        if chain == Chain::bsc_mainnet() {
            return Self::bsc_mainnet_activation_timestamp(fork)
        }
        if chain == Chain::bsc_testnet() {
            return Self::bsc_testnet_activation_timestamp(fork)
        }

        None
    }

    /// Retrieves the activation block for the specified hardfork on the BSC mainnet.
    pub fn bsc_mainnet_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Frontier |
                EthereumHardfork::Homestead |
                EthereumHardfork::Tangerine |
                EthereumHardfork::SpuriousDragon |
                EthereumHardfork::Byzantium |
                EthereumHardfork::Constantinople |
                EthereumHardfork::Petersburg |
                EthereumHardfork::Istanbul |
                EthereumHardfork::MuirGlacier => Some(0),
                EthereumHardfork::Berlin | EthereumHardfork::London => Some(31302048),
                _ => None,
            },
            |fork| match fork {
                Self::Ramanujan | Self::Niels => Some(0),
                Self::MirrorSync => Some(5184000),
                Self::Bruno => Some(13082000),
                Self::Euler => Some(18907621),
                Self::Nano => Some(21962149),
                Self::Moran => Some(22107423),
                Self::Gibbs => Some(23846001),
                Self::Planck => Some(27281024),
                Self::Luban => Some(29020050),
                Self::Plato => Some(30720096),
                Self::Hertz => Some(31302048),
                Self::HertzFix => Some(34140700),
                _ => None,
            },
        )
    }

    /// Retrieves the activation block for the specified hardfork on the BSC testnet.
    pub fn bsc_testnet_activation_block<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Frontier |
                EthereumHardfork::Homestead |
                EthereumHardfork::Tangerine |
                EthereumHardfork::SpuriousDragon |
                EthereumHardfork::Byzantium |
                EthereumHardfork::Constantinople |
                EthereumHardfork::Petersburg |
                EthereumHardfork::Istanbul |
                EthereumHardfork::MuirGlacier => Some(0),
                EthereumHardfork::Berlin | EthereumHardfork::London => Some(31103030),
                _ => None,
            },
            |fork| match fork {
                Self::Ramanujan => Some(1010000),
                Self::Niels => Some(1014369),
                Self::MirrorSync => Some(5582500),
                Self::Bruno => Some(13837000),
                Self::Euler => Some(19203503),
                Self::Gibbs => Some(22800220),
                Self::Nano => Some(23482428),
                Self::Moran => Some(23603940),
                Self::Planck => Some(28196022),
                Self::Luban => Some(29295050),
                Self::Plato => Some(29861024),
                Self::Hertz => Some(31103030),
                Self::HertzFix => Some(35682300),
                _ => None,
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the BSC mainnet.
    pub fn bsc_mainnet_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Shanghai => Some(1705996800),
                EthereumHardfork::Cancun => Some(1718863500),
                _ => None,
            },
            |fork| match fork {
                Self::Kepler => Some(1705996800),
                Self::Feynman | Self::FeynmanFix => Some(1713419340),
                Self::Haber => Some(1718863500),
                _ => None,
            },
        )
    }

    /// Retrieves the activation timestamp for the specified hardfork on the BSC testnet.
    pub fn bsc_testnet_activation_timestamp<H: Hardfork>(fork: H) -> Option<u64> {
        match_hardfork(
            fork,
            |fork| match fork {
                EthereumHardfork::Shanghai => Some(1702972800),
                EthereumHardfork::Cancun => Some(1713330442),
                _ => None,
            },
            |fork| match fork {
                Self::Kepler => Some(1702972800),
                Self::Feynman => Some(1710136800),
                Self::FeynmanFix => Some(1711342800),
                Self::Haber => Some(1716962820),
                Self::HaberFix => Some(1719986788),
                _ => None,
            },
        )
    }

    /// Bsc mainnet list of hardforks.
    pub fn bsc_mainnet() -> ChainHardforks {
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
            (Self::Ramanujan.boxed(), ForkCondition::Block(0)),
            (Self::Niels.boxed(), ForkCondition::Block(0)),
            (Self::MirrorSync.boxed(), ForkCondition::Block(5184000)),
            (Self::Bruno.boxed(), ForkCondition::Block(13082000)),
            (Self::Euler.boxed(), ForkCondition::Block(18907621)),
            (Self::Nano.boxed(), ForkCondition::Block(21962149)),
            (Self::Moran.boxed(), ForkCondition::Block(22107423)),
            (Self::Gibbs.boxed(), ForkCondition::Block(23846001)),
            (Self::Planck.boxed(), ForkCondition::Block(27281024)),
            (Self::Luban.boxed(), ForkCondition::Block(29020050)),
            (Self::Plato.boxed(), ForkCondition::Block(30720096)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(31302048)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(31302048)),
            (Self::Hertz.boxed(), ForkCondition::Block(31302048)),
            (Self::HertzFix.boxed(), ForkCondition::Block(34140700)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1705996800)),
            (Self::Kepler.boxed(), ForkCondition::Timestamp(1705996800)),
            (Self::Feynman.boxed(), ForkCondition::Timestamp(1713419340)),
            (Self::FeynmanFix.boxed(), ForkCondition::Timestamp(1713419340)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1713419340)),
            (Self::Haber.boxed(), ForkCondition::Timestamp(1718863500)),
            (Self::HaberFix.boxed(), ForkCondition::Timestamp(1727316120)),
            (Self::Bohr.boxed(), ForkCondition::Timestamp(1727317200)),
            (Self::Prague.boxed(), ForkCondition::Timestamp(1742436600)),
        ])
    }

    /// Bsc testnet list of hardforks.
    pub fn bsc_testnet() -> ChainHardforks {
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
            (Self::Ramanujan.boxed(), ForkCondition::Block(1010000)),
            (Self::Niels.boxed(), ForkCondition::Block(1014369)),
            (Self::MirrorSync.boxed(), ForkCondition::Block(5582500)),
            (Self::Bruno.boxed(), ForkCondition::Block(13837000)),
            (Self::Euler.boxed(), ForkCondition::Block(19203503)),
            (Self::Gibbs.boxed(), ForkCondition::Block(22800220)),
            (Self::Nano.boxed(), ForkCondition::Block(23482428)),
            (Self::Moran.boxed(), ForkCondition::Block(23603940)),
            (Self::Planck.boxed(), ForkCondition::Block(28196022)),
            (Self::Luban.boxed(), ForkCondition::Block(29295050)),
            (Self::Plato.boxed(), ForkCondition::Block(29861024)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(31103030)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(31103030)),
            (Self::Hertz.boxed(), ForkCondition::Block(31103030)),
            (Self::HertzFix.boxed(), ForkCondition::Block(35682300)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1702972800)),
            (Self::Kepler.boxed(), ForkCondition::Timestamp(1702972800)),
            (Self::Feynman.boxed(), ForkCondition::Timestamp(1710136800)),
            (Self::FeynmanFix.boxed(), ForkCondition::Timestamp(1711342800)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1713330442)),
            (Self::Haber.boxed(), ForkCondition::Timestamp(1716962820)),
            (Self::HaberFix.boxed(), ForkCondition::Timestamp(1719986788)),
            (Self::Bohr.boxed(), ForkCondition::Timestamp(1724116996)),
        ])
    }

    /// Bsc qa list of hardforks.
    pub fn bsc_qa() -> ChainHardforks {
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
            (Self::Ramanujan.boxed(), ForkCondition::Block(0)),
            (Self::Niels.boxed(), ForkCondition::Block(0)),
            (Self::MirrorSync.boxed(), ForkCondition::Block(1)),
            (Self::Bruno.boxed(), ForkCondition::Block(1)),
            (Self::Euler.boxed(), ForkCondition::Block(2)),
            (Self::Nano.boxed(), ForkCondition::Block(3)),
            (Self::Moran.boxed(), ForkCondition::Block(3)),
            (Self::Gibbs.boxed(), ForkCondition::Block(4)),
            (Self::Planck.boxed(), ForkCondition::Block(5)),
            (Self::Luban.boxed(), ForkCondition::Block(6)),
            (Self::Plato.boxed(), ForkCondition::Block(7)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(8)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(8)),
            (Self::Hertz.boxed(), ForkCondition::Block(8)),
            (Self::HertzFix.boxed(), ForkCondition::Block(8)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1722442622)),
            (Self::Kepler.boxed(), ForkCondition::Timestamp(1722442622)),
            (Self::Feynman.boxed(), ForkCondition::Timestamp(1722442622)),
            (Self::FeynmanFix.boxed(), ForkCondition::Timestamp(1722442622)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1722442622)),
            (Self::Haber.boxed(), ForkCondition::Timestamp(1722442622)),
            (Self::HaberFix.boxed(), ForkCondition::Timestamp(1722442622)),
            (Self::Bohr.boxed(), ForkCondition::Timestamp(1722444422)),
        ])
    }
}

/// Match helper method since it's not possible to match on `dyn Hardfork`
fn match_hardfork<H, HF, BHF>(fork: H, hardfork_fn: HF, bsc_hardfork_fn: BHF) -> Option<u64>
where
    H: Hardfork,
    HF: Fn(&EthereumHardfork) -> Option<u64>,
    BHF: Fn(&BscHardfork) -> Option<u64>,
{
    let fork: &dyn Any = &fork;
    if let Some(fork) = fork.downcast_ref::<EthereumHardfork>() {
        return hardfork_fn(fork)
    }
    fork.downcast_ref::<BscHardfork>().and_then(bsc_hardfork_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_hardfork() {
        assert_eq!(BscHardfork::bsc_mainnet_activation_block(EthereumHardfork::Cancun), None);
        assert_eq!(
            BscHardfork::bsc_mainnet_activation_timestamp(EthereumHardfork::Cancun),
            Some(1718863500)
        );
        assert_eq!(BscHardfork::bsc_mainnet_activation_timestamp(BscHardfork::HaberFix), None);
    }
}
