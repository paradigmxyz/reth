use crate::{hardfork, ChainHardforks, ForkCondition, Hardfork};
use alloy_primitives::{uint, U256};
use core::{
    fmt,
    fmt::{Display, Formatter},
    str::FromStr,
};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

hardfork!(
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
    /// Ethereum mainnet list of hardforks.
    pub fn mainnet() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(1150000)),
            (EthereumHardfork::Dao.boxed(), ForkCondition::Block(1920000)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(2463000)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(2675000)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(4370000)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(7280000)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(7280000)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(9069000)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(9200000)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(12244000)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(12965000)),
            (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(13773000)),
            (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(15050000)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD {
                    fork_block: None,
                    total_difficulty: uint!(58_750_000_000_000_000_000_000_U256),
                },
            ),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1681338455)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710338135)),
        ])
    }

    /// Ethereum goerli list of hardforks.
    pub fn goerli() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Dao.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(1561651)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(4460644)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(5062605)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD { fork_block: None, total_difficulty: uint!(10_790_000_U256) },
            ),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1678832736)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1705473120)),
        ])
    }

    /// Ethereum sepolia list of hardforks.
    pub fn sepolia() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Dao.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD {
                    fork_block: Some(1735371),
                    total_difficulty: uint!(17_000_000_000_000_000_U256),
                },
            ),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1677557088)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1706655072)),
        ])
    }

    /// Ethereum holesky list of hardforks.
    pub fn holesky() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Dao.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
            ),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1696000704)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1707305664)),
        ])
    }
}
