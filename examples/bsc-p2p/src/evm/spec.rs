use std::str::FromStr;

use revm::primitives::hardfork::SpecId;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
pub enum BscSpecId {
    FRONTIER = 0,
    FRONTIER_THAWING = 1,
    HOMESTEAD = 2,       // Homestead                            0
    TANGERINE = 3,       // Tangerine Whistle(EIP150)            0
    SPURIOUS_DRAGON = 4, // Spurious Dragon(EIP155, EIP158)      0
    BYZANTIUM = 5,       // Byzantium                            0
    CONSTANTINOPLE = 6,  // Constantinople                       0
    PETERSBURG = 7,      // Petersburg                           0
    ISTANBUL = 8,        // Istanbul                             0
    MUIR_GLACIER = 9,    // Muir Glacier                         0
    RAMANUJAN = 10,      // Ramanujan                            0
    NIELS = 11,          // Niels                                0
    MIRROR_SYNC = 12,    // Mirror Sync                          5184000
    BRUNO = 13,          // Bruno                                13082000
    EULER = 14,          // Euler                                18907621
    NANO = 15,           // Nano                                 21962149
    MORAN = 16,          // Moran                                22107423
    GIBBS = 17,          // Gibbs                                23846001
    PLANCK = 18,         // Planck                               27281024
    LUBAN = 19,          // Luban                                29020050
    PLATO = 20,          // Plato                                30720096
    BERLIN = 21,         // Berlin                               31302048
    LONDON = 22,         // London                               31302048
    HERTZ = 23,          // Hertz                                31302048
    HERTZ_FIX = 24,      // HertzFix                             34140700
    SHANGHAI = 25,       // Shanghai                             timestamp(1705996800)
    KEPLER = 26,         // Kepler                               timestamp(1705996800)
    FEYNMAN = 27,        // Feynman                              timestamp(1713419340)
    FEYNMAN_FIX = 28,    // FeynmanFix                           timestamp(1713419340)
    CANCUN = 29,         // Cancun                               timestamp(1718863500)
    HABER = 30,          // Haber                                timestamp(1718863500)
    HABER_FIX = 31,      // HaberFix                             timestamp(1720591588)
    BOHR = 32,           // Bohr                                 timestamp(1720591588)
    #[default]
    LATEST = u8::MAX,
}

impl BscSpecId {
    pub const fn is_enabled_in(self, other: BscSpecId) -> bool {
        other as u8 <= self as u8
    }

    /// Converts the [`BscSpecId`] into a [`SpecId`].
    pub const fn into_eth_spec(self) -> SpecId {
        match self {
            Self::FRONTIER | Self::FRONTIER_THAWING => SpecId::FRONTIER,
            Self::HOMESTEAD => SpecId::HOMESTEAD,
            Self::TANGERINE => SpecId::TANGERINE,
            Self::SPURIOUS_DRAGON => SpecId::SPURIOUS_DRAGON,
            Self::BYZANTIUM => SpecId::BYZANTIUM,
            Self::CONSTANTINOPLE | Self::PETERSBURG => SpecId::PETERSBURG,
            Self::ISTANBUL |
            Self::MUIR_GLACIER |
            Self::RAMANUJAN |
            Self::NIELS |
            Self::MIRROR_SYNC |
            Self::BRUNO |
            Self::EULER => SpecId::ISTANBUL,
            Self::NANO => SpecId::SHANGHAI,
            Self::MORAN | Self::GIBBS => SpecId::LONDON,
            Self::PLANCK => SpecId::SHANGHAI,
            Self::LUBAN => SpecId::SHANGHAI,
            Self::PLATO => SpecId::SHANGHAI,
            Self::BERLIN => SpecId::BERLIN,
            Self::LONDON => SpecId::LONDON,
            Self::HERTZ | Self::HERTZ_FIX => SpecId::SHANGHAI,
            Self::SHANGHAI => SpecId::SHANGHAI,
            Self::KEPLER => SpecId::SHANGHAI,
            Self::FEYNMAN | Self::FEYNMAN_FIX => SpecId::CANCUN,
            Self::CANCUN => SpecId::CANCUN,
            Self::HABER | Self::HABER_FIX | Self::BOHR => SpecId::CANCUN,
            Self::LATEST => SpecId::CANCUN,
        }
    }
}

impl From<BscSpecId> for SpecId {
    fn from(spec: BscSpecId) -> Self {
        spec.into_eth_spec()
    }
}

/// String identifiers for BSC hardforks
pub mod name {
    pub const FRONTIER: &str = "Frontier";
    pub const FRONTIER_THAWING: &str = "FrontierThawing";
    pub const HOMESTEAD: &str = "Homestead";
    pub const TANGERINE: &str = "Tangerine";
    pub const SPURIOUS_DRAGON: &str = "Spurious";
    pub const BYZANTIUM: &str = "Byzantium";
    pub const CONSTANTINOPLE: &str = "Constantinople";
    pub const PETERSBURG: &str = "Petersburg";
    pub const ISTANBUL: &str = "Istanbul";
    pub const MUIR_GLACIER: &str = "MuirGlacier";
    pub const BERLIN: &str = "Berlin";
    pub const LONDON: &str = "London";
    pub const SHANGHAI: &str = "Shanghai";
    pub const CANCUN: &str = "Cancun";
    pub const RAMANUJAN: &str = "Ramanujan";
    pub const NIELS: &str = "Niels";
    pub const MIRROR_SYNC: &str = "MirrorSync";
    pub const BRUNO: &str = "Bruno";
    pub const EULER: &str = "Euler";
    pub const NANO: &str = "Nano";
    pub const MORAN: &str = "Moran";
    pub const GIBBS: &str = "Gibbs";
    pub const PLANCK: &str = "Planck";
    pub const LUBAN: &str = "Luban";
    pub const PLATO: &str = "Plato";
    pub const HERTZ: &str = "Hertz";
    pub const HERTZ_FIX: &str = "HertzFix";
    pub const KEPLER: &str = "Kepler";
    pub const FEYNMAN: &str = "Feynman";
    pub const FEYNMAN_FIX: &str = "FeynmanFix";
    pub const HABER: &str = "Haber";
    pub const HABER_FIX: &str = "HaberFix";
    pub const BOHR: &str = "Bohr";
}

impl FromStr for BscSpecId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            name::FRONTIER => Self::FRONTIER,
            name::FRONTIER_THAWING => Self::FRONTIER_THAWING,
            name::HOMESTEAD => Self::HOMESTEAD,
            name::TANGERINE => Self::TANGERINE,
            name::SPURIOUS_DRAGON => Self::SPURIOUS_DRAGON,
            name::BYZANTIUM => Self::BYZANTIUM,
            name::CONSTANTINOPLE => Self::CONSTANTINOPLE,
            name::PETERSBURG => Self::PETERSBURG,
            name::ISTANBUL => Self::ISTANBUL,
            name::MUIR_GLACIER => Self::MUIR_GLACIER,
            name::BERLIN => Self::BERLIN,
            name::LONDON => Self::LONDON,
            name::SHANGHAI => Self::SHANGHAI,
            name::CANCUN => Self::CANCUN,
            name::RAMANUJAN => Self::RAMANUJAN,
            name::NIELS => Self::NIELS,
            name::MIRROR_SYNC => Self::MIRROR_SYNC,
            name::BRUNO => Self::BRUNO,
            name::EULER => Self::EULER,
            name::NANO => Self::NANO,
            name::MORAN => Self::MORAN,
            name::GIBBS => Self::GIBBS,
            name::PLANCK => Self::PLANCK,
            name::LUBAN => Self::LUBAN,
            name::PLATO => Self::PLATO,
            name::HERTZ => Self::HERTZ,
            name::HERTZ_FIX => Self::HERTZ_FIX,
            name::KEPLER => Self::KEPLER,
            name::FEYNMAN => Self::FEYNMAN,
            name::FEYNMAN_FIX => Self::FEYNMAN_FIX,
            name::HABER => Self::HABER,
            name::HABER_FIX => Self::HABER_FIX,
            name::BOHR => Self::BOHR,
            _ => return Err(format!("Unknown BSC spec: {}", s)),
        })
    }
}

impl From<BscSpecId> for &'static str {
    fn from(spec_id: BscSpecId) -> Self {
        match spec_id {
            BscSpecId::FRONTIER => name::FRONTIER,
            BscSpecId::FRONTIER_THAWING => name::FRONTIER_THAWING,
            BscSpecId::HOMESTEAD => name::HOMESTEAD,
            BscSpecId::TANGERINE => name::TANGERINE,
            BscSpecId::SPURIOUS_DRAGON => name::SPURIOUS_DRAGON,
            BscSpecId::BYZANTIUM => name::BYZANTIUM,
            BscSpecId::CONSTANTINOPLE => name::CONSTANTINOPLE,
            BscSpecId::PETERSBURG => name::PETERSBURG,
            BscSpecId::ISTANBUL => name::ISTANBUL,
            BscSpecId::MUIR_GLACIER => name::MUIR_GLACIER,
            BscSpecId::BERLIN => name::BERLIN,
            BscSpecId::LONDON => name::LONDON,
            BscSpecId::SHANGHAI => name::SHANGHAI,
            BscSpecId::CANCUN => name::CANCUN,
            BscSpecId::RAMANUJAN => name::RAMANUJAN,
            BscSpecId::NIELS => name::NIELS,
            BscSpecId::MIRROR_SYNC => name::MIRROR_SYNC,
            BscSpecId::BRUNO => name::BRUNO,
            BscSpecId::EULER => name::EULER,
            BscSpecId::NANO => name::NANO,
            BscSpecId::MORAN => name::MORAN,
            BscSpecId::GIBBS => name::GIBBS,
            BscSpecId::PLANCK => name::PLANCK,
            BscSpecId::LUBAN => name::LUBAN,
            BscSpecId::PLATO => name::PLATO,
            BscSpecId::HERTZ => name::HERTZ,
            BscSpecId::HERTZ_FIX => name::HERTZ_FIX,
            BscSpecId::KEPLER => name::KEPLER,
            BscSpecId::FEYNMAN => name::FEYNMAN,
            BscSpecId::FEYNMAN_FIX => name::FEYNMAN_FIX,
            BscSpecId::HABER => name::HABER,
            BscSpecId::HABER_FIX => name::HABER_FIX,
            BscSpecId::BOHR => name::BOHR,
            BscSpecId::LATEST => name::BOHR,
        }
    }
}
