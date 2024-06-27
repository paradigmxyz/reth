use crate::{hardfork, ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};
use alloy_primitives::U256;
use core::{
    fmt,
    fmt::{Display, Formatter},
    str::FromStr,
};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

hardfork!(
    /// The name of an optimism hardfork.
    OptimismHardfork {
        /// Bedrock: <https://blog.oplabs.co/introducing-optimism-bedrock>.
        Bedrock,
        /// Regolith: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#regolith>.
        Regolith,
        /// <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#canyon>.
        Canyon,
        /// Ecotone: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#ecotone>.
        Ecotone,
        /// Fjord: <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/superchain-upgrades.md#fjord>
        Fjord,
    }
);

impl OptimismHardfork {
    /// Optimism mainnet list of hardforks.
    pub fn op_mainnet() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(3950000)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(105235063)),
            (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(105235063)),
            (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(105235063)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD { fork_block: Some(105235063), total_difficulty: U256::ZERO },
            ),
            (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(105235063)),
            (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
            (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
            (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
            (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
        ])
    }

    /// Optimism sepolia list of hardforks.
    pub fn op_sepolia() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
            ),
            (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
            (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
            (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
            (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
            (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
        ])
    }

    /// Base sepolia list of hardforks.
    pub fn base_sepolia() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
            ),
            (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
            (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1699981200)),
            (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1699981200)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1708534800)),
            (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1708534800)),
            (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1716998400)),
        ])
    }

    /// Base mainnet list of hardforks.
    pub fn base_mainnet() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::ArrowGlacier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::GrayGlacier.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
            ),
            (OptimismHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
            (OptimismHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1704992401)),
            (OptimismHardfork::Canyon.boxed(), ForkCondition::Timestamp(1704992401)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1710374401)),
            (OptimismHardfork::Ecotone.boxed(), ForkCondition::Timestamp(1710374401)),
            (OptimismHardfork::Fjord.boxed(), ForkCondition::Timestamp(1720627201)),
        ])
    }
}
