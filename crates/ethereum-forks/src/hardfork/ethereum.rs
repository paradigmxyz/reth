use crate::{define_hardfork_enum, Hardfork};
use alloy_chains::Chain;
use core::{
    fmt,
    fmt::{Display, Formatter},
    str::FromStr,
};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

define_hardfork_enum!(
    /// The name of an Ethereum hardfork.
    EthereumHardfork {
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
        /// Shanghai: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/shanghai.md>.
        Shanghai,
        /// Cancun.
        Cancun,
        /// Prague: <https://github.com/ethereum/execution-specs/blob/master/network-upgrades/mainnet-upgrades/prague.md>
        Prague,
    }
);

impl EthereumHardfork {
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
}
