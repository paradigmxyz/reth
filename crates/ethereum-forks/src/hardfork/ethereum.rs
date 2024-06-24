use crate::{define_hardfork_enum, Hardfork};
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
