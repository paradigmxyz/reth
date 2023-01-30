use serde::{Deserialize, Serialize};

use std::{fmt::Display, str::FromStr};

use crate::{forkkind::ForkDiscriminant, ChainSpec, ForkFilter, ForkId};

#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Hardfork {
    Frontier,
    Homestead,
    Dao,
    Tangerine,
    SpuriousDragon,
    Byzantium,
    Constantinople,
    Petersburg,
    Istanbul,
    Muirglacier,
    Berlin,
    London,
    ArrowGlacier,
    GrayGlacier,
    Paris,
    Shanghai,
}

impl Hardfork {
    /// Compute the forkid for the given [`ChainSpec`].
    ///
    /// This assumes the current hardfork's block number is the current head and uses known future
    /// hardforks from the [`ChainSpec`] to set the forkid's `next` field.
    ///
    /// If the hard fork is not present in the [`ChainSpec`] then `None` is returned.
    pub fn fork_id(&self, chain_spec: &ChainSpec) -> Option<ForkId> {
        chain_spec
            .fork_kind(*self)
            .map(|k| ForkDiscriminant::from_kind(k, chain_spec))
            .map(|d| chain_spec.fork_id(d))
    }

    /// Creates a [`ForkFilter`](crate::ForkFilter) for the given hardfork.
    ///
    /// This assumes the current hardfork's block number is the current head and uses known future
    /// hardforks from the [`ChainSpec`] to initialize the filter.
    ///
    /// This returns `None` if the hardfork is not present in the given [`ChainSpec`].
    pub fn fork_filter(&self, chain_spec: &ChainSpec) -> Option<ForkFilter> {
        chain_spec
            .fork_kind(*self)
            .map(|k| ForkDiscriminant::from_kind(k, chain_spec))
            .map(|d| chain_spec.fork_filter(d))
    }
}

impl FromStr for Hardfork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        let hardfork = match s.as_str() {
            "frontier" | "1" => Hardfork::Frontier,
            "homestead" | "2" => Hardfork::Homestead,
            "dao" | "3" => Hardfork::Dao,
            "tangerine" | "4" => Hardfork::Tangerine,
            "spuriousdragon" | "5" => Hardfork::SpuriousDragon,
            "byzantium" | "6" => Hardfork::Byzantium,
            "constantinople" | "7" => Hardfork::Constantinople,
            "petersburg" | "8" => Hardfork::Petersburg,
            "istanbul" | "9" => Hardfork::Istanbul,
            "muirglacier" | "10" => Hardfork::Muirglacier,
            "berlin" | "11" => Hardfork::Berlin,
            "london" | "12" => Hardfork::London,
            "arrowglacier" | "13" => Hardfork::ArrowGlacier,
            "grayglacier" | "14" => Hardfork::GrayGlacier,
            _ => return Err(format!("Unknown hardfork {s}")),
        };
        Ok(hardfork)
    }
}

impl Display for Hardfork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
