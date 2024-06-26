use crate::{hardforks::ChainHardforks, EthereumHardfork, ForkCondition};
use alloy_primitives::{uint, U256};
use once_cell::sync::Lazy;

/// Ethereum mainnet hardforks
pub static MAINNET_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
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
});

/// Ethereum Goerli hardforks
pub static GOERLI_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
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
});

/// Ethereum Sepolia hardforks
pub static SEPOLIA_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
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
});

/// Ethereum Holesky hardforks
pub static HOLESKY_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
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
});
