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
}

impl Hardfork {
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
    pub fn mainnet_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
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
            Hardfork::Cancun => Some(19426587),

            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Sepolia testnet.
    pub fn sepolia_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Paris => Some(1735371),
            Hardfork::Shanghai => Some(2990908),
            Hardfork::Cancun => Some(5187023),
            Hardfork::Frontier => Some(0),
            Hardfork::Homestead => Some(0),
            Hardfork::Dao => Some(0),
            Hardfork::Tangerine => Some(0),
            Hardfork::SpuriousDragon => Some(0),
            Hardfork::Byzantium => Some(0),
            Hardfork::Constantinople => Some(0),
            Hardfork::Petersburg => Some(0),
            Hardfork::Istanbul => Some(0),
            Hardfork::MuirGlacier => Some(0),
            Hardfork::Berlin => Some(0),
            Hardfork::London => Some(0),
            Hardfork::ArrowGlacier => Some(0),
            Hardfork::GrayGlacier => Some(0),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Arbitrum Sepolia testnet.
    pub fn arbitrum_sepolia_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(0),
            Hardfork::Homestead => Some(0),
            Hardfork::Dao => Some(0),
            Hardfork::Tangerine => Some(0),
            Hardfork::SpuriousDragon => Some(0),
            Hardfork::Byzantium => Some(0),
            Hardfork::Constantinople => Some(0),
            Hardfork::Petersburg => Some(0),
            Hardfork::Istanbul => Some(0),
            Hardfork::MuirGlacier => Some(0),
            Hardfork::Berlin => Some(0),
            Hardfork::London => Some(0),
            Hardfork::ArrowGlacier => Some(0),
            Hardfork::GrayGlacier => Some(0),
            Hardfork::Paris => Some(0),
            Hardfork::Shanghai => Some(10653737),
            // Hardfork::ArbOS11 => Some(10653737),
            Hardfork::Cancun => Some(18683405),
            // Hardfork::ArbOS20Atlas => Some(18683405),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Arbitrum One mainnet.
    pub fn arbitrum_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(0),
            Hardfork::Homestead => Some(0),
            Hardfork::Dao => Some(0),
            Hardfork::Tangerine => Some(0),
            Hardfork::SpuriousDragon => Some(0),
            Hardfork::Byzantium => Some(0),
            Hardfork::Constantinople => Some(0),
            Hardfork::Petersburg => Some(0),
            Hardfork::Istanbul => Some(0),
            Hardfork::MuirGlacier => Some(0),
            Hardfork::Berlin => Some(0),
            Hardfork::London => Some(0),
            Hardfork::ArrowGlacier => Some(0),
            Hardfork::GrayGlacier => Some(0),
            Hardfork::Paris => Some(0),
            Hardfork::Shanghai => Some(184097479),
            // Hardfork::ArbOS11 => Some(184097479),
            Hardfork::Cancun => Some(190301729),
            // Hardfork::ArbOS20Atlas => Some(190301729),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Base Sepolia testnet.
    #[cfg(feature = "optimism")]
    pub fn base_sepolia_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(0),
            Hardfork::Homestead => Some(0),
            Hardfork::Dao => Some(0),
            Hardfork::Tangerine => Some(0),
            Hardfork::SpuriousDragon => Some(0),
            Hardfork::Byzantium => Some(0),
            Hardfork::Constantinople => Some(0),
            Hardfork::Petersburg => Some(0),
            Hardfork::Istanbul => Some(0),
            Hardfork::MuirGlacier => Some(0),
            Hardfork::Berlin => Some(0),
            Hardfork::London => Some(0),
            Hardfork::ArrowGlacier => Some(0),
            Hardfork::GrayGlacier => Some(0),
            Hardfork::Paris => Some(0),
            Hardfork::Bedrock => Some(0),
            Hardfork::Regolith => Some(0),
            Hardfork::Shanghai => Some(2106456),
            Hardfork::Canyon => Some(2106456),
            Hardfork::Cancun => Some(6383256),
            Hardfork::Ecotone => Some(6383256),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the Base mainnet.
    #[cfg(feature = "optimism")]
    pub fn base_mainnet_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(0),
            Hardfork::Homestead => Some(0),
            Hardfork::Dao => Some(0),
            Hardfork::Tangerine => Some(0),
            Hardfork::SpuriousDragon => Some(0),
            Hardfork::Byzantium => Some(0),
            Hardfork::Constantinople => Some(0),
            Hardfork::Petersburg => Some(0),
            Hardfork::Istanbul => Some(0),
            Hardfork::MuirGlacier => Some(0),
            Hardfork::Berlin => Some(0),
            Hardfork::London => Some(0),
            Hardfork::ArrowGlacier => Some(0),
            Hardfork::GrayGlacier => Some(0),
            Hardfork::Paris => Some(0),
            Hardfork::Bedrock => Some(0),
            Hardfork::Regolith => Some(0),
            Hardfork::Shanghai => Some(9101527),
            Hardfork::Canyon => Some(9101527),
            Hardfork::Cancun => Some(11188936),
            Hardfork::Ecotone => Some(11188936),
            _ => None,
        }
    }

    /// Retrieves the activation block for the specified hardfork on the holesky testnet.
    fn holesky_activation_block(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Dao => Some(0),
            Hardfork::Tangerine => Some(0),
            Hardfork::SpuriousDragon => Some(0),
            Hardfork::Byzantium => Some(0),
            Hardfork::Constantinople => Some(0),
            Hardfork::Petersburg => Some(0),
            Hardfork::Istanbul => Some(0),
            Hardfork::MuirGlacier => Some(0),
            Hardfork::Berlin => Some(0),
            Hardfork::London => Some(0),
            Hardfork::ArrowGlacier => Some(0),
            Hardfork::GrayGlacier => Some(0),
            Hardfork::Paris => Some(0),
            Hardfork::Shanghai => Some(6698),
            Hardfork::Cancun => Some(894733),
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
    pub fn mainnet_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
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
            Hardfork::Cancun => Some(1710338135),

            // upcoming hardforks
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Sepolia testnet.
    pub fn sepolia_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(1633267481),
            Hardfork::Homestead => Some(1633267481),
            Hardfork::Dao => Some(1633267481),
            Hardfork::Tangerine => Some(1633267481),
            Hardfork::SpuriousDragon => Some(1633267481),
            Hardfork::Byzantium => Some(1633267481),
            Hardfork::Constantinople => Some(1633267481),
            Hardfork::Petersburg => Some(1633267481),
            Hardfork::Istanbul => Some(1633267481),
            Hardfork::MuirGlacier => Some(1633267481),
            Hardfork::Berlin => Some(1633267481),
            Hardfork::London => Some(1633267481),
            Hardfork::ArrowGlacier => Some(1633267481),
            Hardfork::GrayGlacier => Some(1633267481),
            Hardfork::Paris => Some(1633267481),
            Hardfork::Shanghai => Some(1677557088),
            Hardfork::Cancun => Some(1706655072),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Holesky testnet.
    pub fn holesky_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Shanghai => Some(1696000704),
            Hardfork::Cancun => Some(1707305664),
            Hardfork::Frontier => Some(1695902100),
            Hardfork::Homestead => Some(1695902100),
            Hardfork::Dao => Some(1695902100),
            Hardfork::Tangerine => Some(1695902100),
            Hardfork::SpuriousDragon => Some(1695902100),
            Hardfork::Byzantium => Some(1695902100),
            Hardfork::Constantinople => Some(1695902100),
            Hardfork::Petersburg => Some(1695902100),
            Hardfork::Istanbul => Some(1695902100),
            Hardfork::MuirGlacier => Some(1695902100),
            Hardfork::Berlin => Some(1695902100),
            Hardfork::London => Some(1695902100),
            Hardfork::ArrowGlacier => Some(1695902100),
            Hardfork::GrayGlacier => Some(1695902100),
            Hardfork::Paris => Some(1695902100),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Arbitrum Sepolia
    /// testnet.
    pub fn arbitrum_sepolia_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(1692726996),
            Hardfork::Homestead => Some(1692726996),
            Hardfork::Dao => Some(1692726996),
            Hardfork::Tangerine => Some(1692726996),
            Hardfork::SpuriousDragon => Some(1692726996),
            Hardfork::Byzantium => Some(1692726996),
            Hardfork::Constantinople => Some(1692726996),
            Hardfork::Petersburg => Some(1692726996),
            Hardfork::Istanbul => Some(1692726996),
            Hardfork::MuirGlacier => Some(1692726996),
            Hardfork::Berlin => Some(1692726996),
            Hardfork::London => Some(1692726996),
            Hardfork::ArrowGlacier => Some(1692726996),
            Hardfork::GrayGlacier => Some(1692726996),
            Hardfork::Paris => Some(1692726996),
            Hardfork::Shanghai => Some(1706634000),
            // Hardfork::ArbOS11 => Some(1706634000),
            Hardfork::Cancun => Some(1709229600),
            // Hardfork::ArbOS20Atlas => Some(1709229600),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Arbitrum One mainnet.
    pub fn arbitrum_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(1622240000),
            Hardfork::Homestead => Some(1622240000),
            Hardfork::Dao => Some(1622240000),
            Hardfork::Tangerine => Some(1622240000),
            Hardfork::SpuriousDragon => Some(1622240000),
            Hardfork::Byzantium => Some(1622240000),
            Hardfork::Constantinople => Some(1622240000),
            Hardfork::Petersburg => Some(1622240000),
            Hardfork::Istanbul => Some(1622240000),
            Hardfork::MuirGlacier => Some(1622240000),
            Hardfork::Berlin => Some(1622240000),
            Hardfork::London => Some(1622240000),
            Hardfork::ArrowGlacier => Some(1622240000),
            Hardfork::GrayGlacier => Some(1622240000),
            Hardfork::Paris => Some(1622240000),
            Hardfork::Shanghai => Some(1708804873),
            // Hardfork::ArbOS11 => Some(1708804873),
            Hardfork::Cancun => Some(1710424089),
            // Hardfork::ArbOS20Atlas => Some(1710424089),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base Sepolia testnet.
    #[cfg(feature = "optimism")]
    pub fn base_sepolia_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(1695768288),
            Hardfork::Homestead => Some(1695768288),
            Hardfork::Dao => Some(1695768288),
            Hardfork::Tangerine => Some(1695768288),
            Hardfork::SpuriousDragon => Some(1695768288),
            Hardfork::Byzantium => Some(1695768288),
            Hardfork::Constantinople => Some(1695768288),
            Hardfork::Petersburg => Some(1695768288),
            Hardfork::Istanbul => Some(1695768288),
            Hardfork::MuirGlacier => Some(1695768288),
            Hardfork::Berlin => Some(1695768288),
            Hardfork::London => Some(1695768288),
            Hardfork::ArrowGlacier => Some(1695768288),
            Hardfork::GrayGlacier => Some(1695768288),
            Hardfork::Paris => Some(1695768288),
            Hardfork::Bedrock => Some(1695768288),
            Hardfork::Regolith => Some(1695768288),
            Hardfork::Shanghai => Some(1699981200),
            Hardfork::Canyon => Some(1699981200),
            Hardfork::Cancun => Some(1708534800),
            Hardfork::Ecotone => Some(1708534800),
            _ => None,
        }
    }

    /// Retrieves the activation timestamp for the specified hardfork on the Base mainnet.
    #[cfg(feature = "optimism")]
    pub fn base_mainnet_activation_timestamp(&self) -> Option<u64> {
        #[allow(unreachable_patterns)]
        match self {
            Hardfork::Frontier => Some(1686789347),
            Hardfork::Homestead => Some(1686789347),
            Hardfork::Dao => Some(1686789347),
            Hardfork::Tangerine => Some(1686789347),
            Hardfork::SpuriousDragon => Some(1686789347),
            Hardfork::Byzantium => Some(1686789347),
            Hardfork::Constantinople => Some(1686789347),
            Hardfork::Petersburg => Some(1686789347),
            Hardfork::Istanbul => Some(1686789347),
            Hardfork::MuirGlacier => Some(1686789347),
            Hardfork::Berlin => Some(1686789347),
            Hardfork::London => Some(1686789347),
            Hardfork::ArrowGlacier => Some(1686789347),
            Hardfork::GrayGlacier => Some(1686789347),
            Hardfork::Paris => Some(1686789347),
            Hardfork::Bedrock => Some(1686789347),
            Hardfork::Regolith => Some(1686789347),
            Hardfork::Shanghai => Some(1704992401),
            Hardfork::Canyon => Some(1704992401),
            Hardfork::Cancun => Some(1710374401),
            Hardfork::Ecotone => Some(1710374401),
            _ => None,
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
            // "arbos11" => Hardfork::ArbOS11,
            // "arbos20atlas" => Hardfork::ArbOS20Atlas,
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
