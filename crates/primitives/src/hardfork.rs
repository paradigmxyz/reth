use serde::{Deserialize, Serialize};

use std::{fmt::Display, str::FromStr};

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
    Shanghai,
    Latest,
}

impl Hardfork {
    /// This returns all known hardforks in order.
    pub fn all_forks() -> Vec<Self> {
        vec![
            Hardfork::Homestead,
            Hardfork::Dao,
            Hardfork::Tangerine,
            Hardfork::SpuriousDragon,
            Hardfork::Byzantium,
            Hardfork::Constantinople, /* petersburg is skipped because it's the same block num
                                       * as constantinople */
            Hardfork::Istanbul,
            Hardfork::Muirglacier,
            Hardfork::Berlin,
            Hardfork::London,
            Hardfork::ArrowGlacier,
            Hardfork::GrayGlacier,
        ]
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
            "grayglacier" => Hardfork::GrayGlacier,
            "latest" | "14" => Hardfork::Latest,
            _ => return Err(format!("Unknown hardfork {s}")),
        };
        Ok(hardfork)
    }
}

impl Default for Hardfork {
    fn default() -> Self {
        Hardfork::Latest
    }
}

impl Display for Hardfork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
