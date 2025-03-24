use reth_chainspec::{hardfork, ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};

hardfork!(
    /// The name of a bsc hardfork.
    ///
    /// When building a list of hardforks for a chain, it's still expected to mix with [`EthereumHardfork`].
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
    /// Bsc mainnet list of hardforks.
    pub(crate) fn bsc_mainnet() -> ChainHardforks {
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
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1718863500)),
            (Self::Haber.boxed(), ForkCondition::Timestamp(1718863500)),
            (Self::HaberFix.boxed(), ForkCondition::Timestamp(1727316120)),
            (Self::Bohr.boxed(), ForkCondition::Timestamp(1727317200)),
            (Self::Pascal.boxed(), ForkCondition::Timestamp(1742436600)),
            (Self::Prague.boxed(), ForkCondition::Timestamp(1742436600)),
        ])
    }
}
