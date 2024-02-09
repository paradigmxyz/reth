use alloy_chains::Chain;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{fmt::Display, str::FromStr};

/// Represents the consensus type of a blockchain fork.
///
/// This enum defines two variants: `ProofOfWork` for hardforks that use a proof-of-work consensus
/// mechanism, and `ProofOfStake` for hardforks that use a proof-of-stake consensus mechanism.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ConsensusType {
    /// Indicates a proof-of-work consensus mechanism.
    ProofOfWork,
    /// Indicates a proof-of-stake consensus mechanism.
    ProofOfStake,
}

/// The name of an Ethereum hardfork.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
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
    /// Ecotone
    #[cfg(feature = "optimism")]
    Ecotone,
}

impl Hardfork {
    /// Retrieves the activation block for the specified hardfork on the Ethereum mainnet.
    pub fn mainnet_activation_block(&self, chain: Chain) -> Option<u64> {
        if chain != Chain::mainnet() {
            return None
        }
        match self {
            Hardfork::Frontier => Some(0),
            Hardfork::Homestead => Some(1150000),
            Hardfork::Dao => Some(1920000),
            Hardfork::Tangerine => Some(2463000),
            Hardfork::SpuriousDragon => Some(2675000),
            Hardfork::Byzantium => Some(4370000),
            Hardfork::Constantinople => Some(7280000),
            Hardfork::Petersburg => Some(7280000),
            Hardfork::Istanbul => Some(9069000),
            Hardfork::MuirGlacier => Some(9200000),
            Hardfork::Berlin => Some(12244000),
            Hardfork::London => Some(12965000),
            Hardfork::ArrowGlacier => Some(13773000),
            Hardfork::GrayGlacier => Some(15050000),
            Hardfork::Paris => Some(15537394),
            Hardfork::Shanghai => Some(17034870),

            // upcoming hardforks
            Hardfork::Cancun => None,

            // optimism hardforks
            #[cfg(feature = "optimism")]
            Hardfork::Bedrock => None,
            #[cfg(feature = "optimism")]
            Hardfork::Regolith => None,
            #[cfg(feature = "optimism")]
            Hardfork::Canyon => None,
            #[cfg(feature = "optimism")]
            Hardfork::Ecotone => None,
        }
    }

    /// Retrieves the consensus type for the specified hardfork.
    pub fn consensus_type(&self) -> ConsensusType {
        if *self >= Hardfork::Paris {
            ConsensusType::ProofOfStake
        } else {
            ConsensusType::ProofOfWork
        }
    }

    /// Checks if the hardfork uses Proof of Stake consensus.
    pub fn is_proof_of_stake(&self) -> bool {
        matches!(self.consensus_type(), ConsensusType::ProofOfStake)
    }

    /// Checks if the hardfork uses Proof of Work consensus.
    pub fn is_proof_of_work(&self) -> bool {
        matches!(self.consensus_type(), ConsensusType::ProofOfWork)
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    pub fn activation_timestamp(&self, chain: Chain) -> Option<u64> {
        if chain != Chain::mainnet() {
            return None
        }
        self.mainnet_activation_timestamp()
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Ethereum mainnet.
    pub fn mainnet_activation_timestamp(&self) -> Option<u64> {
        match self {
            Hardfork::Frontier => Some(1438226773),
            Hardfork::Homestead => Some(1457938193),
            Hardfork::Dao => Some(1468977640),
            Hardfork::Tangerine => Some(1476753571),
            Hardfork::SpuriousDragon => Some(1479788144),
            Hardfork::Byzantium => Some(1508131331),
            Hardfork::Constantinople => Some(1551340324),
            Hardfork::Petersburg => Some(1551340324),
            Hardfork::Istanbul => Some(1575807909),
            Hardfork::MuirGlacier => Some(1577953849),
            Hardfork::Berlin => Some(1618481223),
            Hardfork::London => Some(1628166822),
            Hardfork::ArrowGlacier => Some(1639036523),
            Hardfork::GrayGlacier => Some(1656586444),
            Hardfork::Paris => Some(1663224162),
            Hardfork::Shanghai => Some(1681338455),

            // upcoming hardforks
            Hardfork::Cancun => None,

            // optimism hardforks
            #[cfg(feature = "optimism")]
            Hardfork::Bedrock => None,
            #[cfg(feature = "optimism")]
            Hardfork::Regolith => None,
            #[cfg(feature = "optimism")]
            Hardfork::Canyon => None,
            #[cfg(feature = "optimism")]
            Hardfork::Ecotone => None,
        }
    }
}

impl FromStr for Hardfork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
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
            #[cfg(feature = "optimism")]
            "ecotone" => Hardfork::Ecotone,
            _ => return Err(format!("Unknown hardfork: {s}")),
        })
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
        let hardfork_str = ["beDrOck", "rEgOlITH", "cAnYoN", "eCoToNe"];
        let expected_hardforks =
            [Hardfork::Bedrock, Hardfork::Regolith, Hardfork::Canyon, Hardfork::Ecotone];

        let hardforks: Vec<Hardfork> =
            hardfork_str.iter().map(|h| Hardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(Hardfork::from_str("not a hardfork").is_err());
    }

    #[test]
    fn check_consensus_type() {
        let pow_hardforks = [
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
        ];

        let pos_hardforks = [Hardfork::Paris, Hardfork::Shanghai, Hardfork::Cancun];

        #[cfg(feature = "optimism")]
        let op_hardforks =
            [Hardfork::Bedrock, Hardfork::Regolith, Hardfork::Canyon, Hardfork::Ecotone];

        for hardfork in pow_hardforks.iter() {
            assert_eq!(hardfork.consensus_type(), ConsensusType::ProofOfWork);
            assert!(!hardfork.is_proof_of_stake());
            assert!(hardfork.is_proof_of_work());
        }

        for hardfork in pos_hardforks.iter() {
            assert_eq!(hardfork.consensus_type(), ConsensusType::ProofOfStake);
            assert!(hardfork.is_proof_of_stake());
            assert!(!hardfork.is_proof_of_work());
        }

        #[cfg(feature = "optimism")]
        for hardfork in op_hardforks.iter() {
            assert_eq!(hardfork.consensus_type(), ConsensusType::ProofOfStake);
            assert!(hardfork.is_proof_of_stake());
            assert!(!hardfork.is_proof_of_work());
        }
    }
}
