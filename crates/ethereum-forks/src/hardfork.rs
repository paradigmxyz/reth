use serde::{Deserialize, Serialize};

use std::{fmt::Display, str::FromStr};

/// The name of an Ethereum hardfork.
#[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Hardfork {
    /// Frontier.
    Frontier,
    /// Homestead.
    Homestead,
    /// The DAO fork.
    Dao,
    /// Tangerine.
    Tangerine,
    /// Spurious Dragon.
    SpuriousDragon,
    /// Byzantium.
    Byzantium,
    /// Constantinople.
    Constantinople,
    /// Petersburg.
    Petersburg,
    /// Istanbul.
    Istanbul,
    /// Muir Glacier.
    MuirGlacier,
    /// Berlin.
    Berlin,
    /// London.
    London,
    /// Arrow Glacier.
    ArrowGlacier,
    /// Gray Glacier.
    GrayGlacier,
    /// Paris.
    Paris,
    /// Bedrock.
    #[cfg(feature = "optimism")]
    Bedrock,
    /// Regolith
    #[cfg(feature = "optimism")]
    Regolith,
    /// Shanghai.
    Shanghai,
    /// Canyon
    #[cfg(feature = "optimism")]
    Canyon,
    /// Cancun.
    Cancun,
}

impl FromStr for Hardfork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        let hardfork = match s.as_str() {
            "frontier" => Hardfork::Frontier,
            "homestead" => Hardfork::Homestead,
            "dao" => Hardfork::Dao,
            "tangerine" => Hardfork::Tangerine,
            "spuriousdragon" => Hardfork::SpuriousDragon,
            "byzantium" => Hardfork::Byzantium,
            "constantinople" => Hardfork::Constantinople,
            "petersburg" => Hardfork::Petersburg,
            "istanbul" => Hardfork::Istanbul,
            "muirglacier" => Hardfork::MuirGlacier,
            "berlin" => Hardfork::Berlin,
            "london" => Hardfork::London,
            "arrowglacier" => Hardfork::ArrowGlacier,
            "grayglacier" => Hardfork::GrayGlacier,
            "paris" => Hardfork::Paris,
            "shanghai" => Hardfork::Shanghai,
            "cancun" => Hardfork::Cancun,
            #[cfg(feature = "optimism")]
            "bedrock" => Hardfork::Bedrock,
            #[cfg(feature = "optimism")]
            "regolith" => Hardfork::Regolith,
            #[cfg(feature = "optimism")]
            "canyon" => Hardfork::Canyon,
            _ => return Err(format!("Unknown hardfork: {s}")),
        };
        Ok(hardfork)
    }
}

impl Display for Hardfork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_hardfork_from_str() {
        let hardfork_str = [
            "frOntier",
            "homEstead",
            "dao",
            "tAngerIne",
            "spurIousdrAgon",
            "byzAntium",
            "constantinople",
            "petersburg",
            "istanbul",
            "muirglacier",
            "bErlin",
            "lonDon",
            "arrowglacier",
            "grayglacier",
            "PARIS",
            "ShAnGhAI",
            "CaNcUn",
        ];
        let expected_hardforks = [
            Hardfork::Frontier,
            Hardfork::Homestead,
            Hardfork::Dao,
            Hardfork::Tangerine,
            Hardfork::SpuriousDragon,
            Hardfork::Byzantium,
            Hardfork::Constantinople,
            Hardfork::Petersburg,
            Hardfork::Istanbul,
            Hardfork::MuirGlacier,
            Hardfork::Berlin,
            Hardfork::London,
            Hardfork::ArrowGlacier,
            Hardfork::GrayGlacier,
            Hardfork::Paris,
            Hardfork::Shanghai,
            Hardfork::Cancun,
        ];

        let hardforks: Vec<Hardfork> =
            hardfork_str.iter().map(|h| Hardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    #[cfg(feature = "optimism")]
    fn check_op_hardfork_from_str() {
        let hardfork_str = ["beDrOck", "rEgOlITH", "cAnYoN"];
        let expected_hardforks = [Hardfork::Bedrock, Hardfork::Regolith, Hardfork::Canyon];

        let hardforks: Vec<Hardfork> =
            hardfork_str.iter().map(|h| Hardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(Hardfork::from_str("not a hardfork").is_err());
    }
}
