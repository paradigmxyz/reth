use alloy_chains::Chain;
use core::{
    fmt,
    fmt::{Display, Formatter},
    str::FromStr,
};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "std"))]
use alloc::{format, string::String};

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
    /// Frontier: <https://blog.ethereum.org/2015/03/03/ethereum-launch-process>.
    Frontier,
    /// Homestead: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/homestead.md>.
    Homestead,
    /// The DAO fork: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/dao-fork.md>.
    Dao,
    /// Tangerine: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/tangerine-whistle.md>.
    Tangerine,
    /// Spurious Dragon: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/spurious-dragon.md>.
    SpuriousDragon,
    /// Byzantium: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/byzantium.md>.
    Byzantium,
    /// Constantinople: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/constantinople.md>.
    Constantinople,
    /// Petersburg: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/petersburg.md>.
    Petersburg,
    /// Istanbul: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/istanbul.md>.
    Istanbul,
    /// Muir Glacier: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/muir-glacier.md>.
    MuirGlacier,
    /// Berlin: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/berlin.md>.
    Berlin,
    /// London: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/london.md>.
    London,
    /// Arrow Glacier: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/arrow-glacier.md>.
    ArrowGlacier,
    /// Gray Glacier: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/gray-glacier.md>.
    GrayGlacier,
    /// Paris: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/paris.md>.
    Paris,
    /// Bedrock: <https://blog.oplabs.co/introducing-optimism-bedrock>.
    #[cfg(feature = "optimism")]
    Bedrock,
    /// Regolith: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#regolith>.
    #[cfg(feature = "optimism")]
    Regolith,
    /// Shanghai: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/shanghai.md>.
    Shanghai,
    /// Canyon:
    /// <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#canyon>.
    #[cfg(feature = "optimism")]
    Canyon,
    // ArbOS11,
    /// Cancun.
    Cancun,
    /// Ecotone: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#ecotone>.
    #[cfg(feature = "optimism")]
    Ecotone,
    // ArbOS20Atlas,

    // Upcoming
    /// Prague: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/prague.md>
    Prague,
    /// Fjord: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#fjord>
    #[cfg(feature = "optimism")]
    Fjord,
}

impl Hardfork {
    /// Retrieves the consensus type for the specified hardfork.
    pub fn consensus_type(&self) -> ConsensusType {
        if *self >= Self::Paris {
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

    /// Retrieves the activation block for the specified hardfork on the given chain.
    pub fn activation_block(&self, chain: Chain) -> Option<u64> {
        if chain == Chain::mainnet() {
            return self.mainnet_activation_block()
        }
        if chain == Chain::sepolia() {
            return self.sepolia_activation_block()
        }
        if chain == Chain::holesky() {
            return self.holesky_activation_block()
        }

        #[cfg(feature = "optimism")]
        {
            if chain == Chain::base_sepolia() {
                return self.base_sepolia_activation_block()
            }
            if chain == Chain::base_mainnet() {
                return self.base_mainnet_activation_block()
            }
        }

        None
    }

    /// Retrieves the activation block for the specified hardfork on the Ethereum mainnet.
    pub const fn mainnet_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier => Some(0),
            Self::Homestead => Some(1150000),
            Self::Dao => Some(1920000),
            Self::Tangerine => Some(2463000),
            Self::SpuriousDragon => Some(2675000),
            Self::Byzantium => Some(4370000),
            Self::Constantinople | Self::Petersburg => Some(7280000),
            Self::Istanbul => Some(9069000),
            Self::MuirGlacier => Some(9200000),
            Self::Berlin => Some(12244000),
            Self::London => Some(12965000),
            Self::ArrowGlacier => Some(13773000),
            Self::GrayGlacier => Some(15050000),
            Self::Paris => Some(15537394),
            Self::Shanghai => Some(17034870),
            Self::Cancun => Some(19426587),

            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Sepolia testnet.
    pub const fn sepolia_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Paris => Some(1735371),
            Self::Shanghai => Some(2990908),
            Self::Cancun => Some(5187023),
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier => Some(0),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Arbitrum Sepolia testnet.
    pub const fn arbitrum_sepolia_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(0),
            Self::Shanghai => Some(10653737),
            // Hardfork::ArbOS11 => Some(10653737),
            Self::Cancun => Some(18683405),
            // Hardfork::ArbOS20Atlas => Some(18683405),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Arbitrum One mainnet.
    pub const fn arbitrum_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(0),
            Self::Shanghai => Some(184097479),
            // Hardfork::ArbOS11 => Some(184097479),
            Self::Cancun => Some(190301729),
            // Hardfork::ArbOS20Atlas => Some(190301729),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Base Sepolia testnet.
    #[cfg(feature = "optimism")]
    pub const fn base_sepolia_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris |
            Self::Bedrock |
            Self::Regolith => Some(0),
            Self::Shanghai | Self::Canyon => Some(2106456),
            Self::Cancun | Self::Ecotone => Some(6383256),
            Self::Fjord => Some(10615056),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Base mainnet.
    #[cfg(feature = "optimism")]
    pub const fn base_mainnet_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris |
            Self::Bedrock |
            Self::Regolith => Some(0),
            Self::Shanghai | Self::Canyon => Some(9101527),
            Self::Cancun | Self::Ecotone => Some(11188936),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the holesky testnet.
    const fn holesky_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(0),
            Self::Shanghai => Some(6698),
            Self::Cancun => Some(894733),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the given chain.
    pub fn activation_timestamp(&self, chain: Chain) -> Option<u64> {
        if chain == Chain::mainnet() {
            return self.mainnet_activation_timestamp()
        }
        if chain == Chain::sepolia() {
            return self.sepolia_activation_timestamp()
        }
        if chain == Chain::holesky() {
            return self.holesky_activation_timestamp()
        }
        #[cfg(feature = "optimism")]
        {
            if chain == Chain::base_sepolia() {
                return self.base_sepolia_activation_timestamp()
            }
            if chain == Chain::base_mainnet() {
                return self.base_mainnet_activation_timestamp()
            }
        }

        None
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Ethereum mainnet.
    pub const fn mainnet_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier => Some(1438226773),
            Self::Homestead => Some(1457938193),
            Self::Dao => Some(1468977640),
            Self::Tangerine => Some(1476753571),
            Self::SpuriousDragon => Some(1479788144),
            Self::Byzantium => Some(1508131331),
            Self::Constantinople | Self::Petersburg => Some(1551340324),
            Self::Istanbul => Some(1575807909),
            Self::MuirGlacier => Some(1577953849),
            Self::Berlin => Some(1618481223),
            Self::London => Some(1628166822),
            Self::ArrowGlacier => Some(1639036523),
            Self::GrayGlacier => Some(1656586444),
            Self::Paris => Some(1663224162),
            Self::Shanghai => Some(1681338455),
            Self::Cancun => Some(1710338135),

            // upcoming hardforks
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Sepolia testnet.
    pub const fn sepolia_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(1633267481),
            Self::Shanghai => Some(1677557088),
            Self::Cancun => Some(1706655072),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Holesky testnet.
    pub const fn holesky_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Shanghai => Some(1696000704),
            Self::Cancun => Some(1707305664),
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(1695902100),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Arbitrum Sepolia
    /// testnet.
    pub const fn arbitrum_sepolia_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(1692726996),
            Self::Shanghai => Some(1706634000),
            // Hardfork::ArbOS11 => Some(1706634000),
            Self::Cancun => Some(1709229600),
            // Hardfork::ArbOS20Atlas => Some(1709229600),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Arbitrum One mainnet.
    pub const fn arbitrum_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris => Some(1622240000),
            Self::Shanghai => Some(1708804873),
            // Hardfork::ArbOS11 => Some(1708804873),
            Self::Cancun => Some(1710424089),
            // Hardfork::ArbOS20Atlas => Some(1710424089),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base Sepolia testnet.
    #[cfg(feature = "optimism")]
    pub const fn base_sepolia_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris |
            Self::Bedrock |
            Self::Regolith => Some(1695768288),
            Self::Shanghai | Self::Canyon => Some(1699981200),
            Self::Cancun | Self::Ecotone => Some(1708534800),
            Self::Fjord => Some(1716998400),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base mainnet.
    #[cfg(feature = "optimism")]
    pub const fn base_mainnet_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Self::Frontier |
            Self::Homestead |
            Self::Dao |
            Self::Tangerine |
            Self::SpuriousDragon |
            Self::Byzantium |
            Self::Constantinople |
            Self::Petersburg |
            Self::Istanbul |
            Self::MuirGlacier |
            Self::Berlin |
            Self::London |
            Self::ArrowGlacier |
            Self::GrayGlacier |
            Self::Paris |
            Self::Bedrock |
            Self::Regolith => Some(1686789347),
            Self::Shanghai | Self::Canyon => Some(1704992401),
            Self::Cancun | Self::Ecotone => Some(1710374401),
            Self::Fjord => Some(1720627201),
            _ => None,
        }
    }
}

impl FromStr for Hardfork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "frontier" => Self::Frontier,
            "homestead" => Self::Homestead,
            "dao" => Self::Dao,
            "tangerine" => Self::Tangerine,
            "spuriousdragon" => Self::SpuriousDragon,
            "byzantium" => Self::Byzantium,
            "constantinople" => Self::Constantinople,
            "petersburg" => Self::Petersburg,
            "istanbul" => Self::Istanbul,
            "muirglacier" => Self::MuirGlacier,
            "berlin" => Self::Berlin,
            "london" => Self::London,
            "arrowglacier" => Self::ArrowGlacier,
            "grayglacier" => Self::GrayGlacier,
            "paris" => Self::Paris,
            "shanghai" => Self::Shanghai,
            "cancun" => Self::Cancun,
            #[cfg(feature = "optimism")]
            "bedrock" => Self::Bedrock,
            #[cfg(feature = "optimism")]
            "regolith" => Self::Regolith,
            #[cfg(feature = "optimism")]
            "canyon" => Self::Canyon,
            #[cfg(feature = "optimism")]
            "ecotone" => Self::Ecotone,
            #[cfg(feature = "optimism")]
            "fjord" => Self::Fjord,
            "prague" => Self::Prague,
            // "arbos11" => Hardfork::ArbOS11,
            // "arbos20atlas" => Hardfork::ArbOS20Atlas,
            _ => return Err(format!("Unknown hardfork: {s}")),
        })
    }
}

impl Display for Hardfork {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
            "PrAguE",
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
            Hardfork::Prague,
        ];

        let hardforks: Vec<Hardfork> =
            hardfork_str.iter().map(|h| Hardfork::from_str(h).unwrap()).collect();

        assert_eq!(hardforks, expected_hardforks);
    }

    #[test]
    #[cfg(feature = "optimism")]
    fn check_op_hardfork_from_str() {
        let hardfork_str = ["beDrOck", "rEgOlITH", "cAnYoN", "eCoToNe", "FJorD"];
        let expected_hardforks = [
            Hardfork::Bedrock,
            Hardfork::Regolith,
            Hardfork::Canyon,
            Hardfork::Ecotone,
            Hardfork::Fjord,
        ];

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
        let op_hardforks = [
            Hardfork::Bedrock,
            Hardfork::Regolith,
            Hardfork::Canyon,
            Hardfork::Ecotone,
            Hardfork::Fjord,
        ];

        for hardfork in &pow_hardforks {
            assert_eq!(hardfork.consensus_type(), ConsensusType::ProofOfWork);
            assert!(!hardfork.is_proof_of_stake());
            assert!(hardfork.is_proof_of_work());
        }

        for hardfork in &pos_hardforks {
            assert_eq!(hardfork.consensus_type(), ConsensusType::ProofOfStake);
            assert!(hardfork.is_proof_of_stake());
            assert!(!hardfork.is_proof_of_work());
        }

        #[cfg(feature = "optimism")]
        for hardfork in &op_hardforks {
            assert_eq!(hardfork.consensus_type(), ConsensusType::ProofOfStake);
            assert!(hardfork.is_proof_of_stake());
            assert!(!hardfork.is_proof_of_work());
        }
    }
}
