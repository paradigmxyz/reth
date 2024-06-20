use crate::{ForkCondition, Hardfork};
use alloy_primitives::U256;
use once_cell::sync::Lazy;
use std::collections::BTreeMap;

/// Ethereum mainnet hardforks
pub static MAINNET_HARDFORKS: Lazy<BTreeMap<Hardfork, ForkCondition>> = Lazy::new(|| {
    BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Homestead, ForkCondition::Block(1150000)),
        (Hardfork::Dao, ForkCondition::Block(1920000)),
        (Hardfork::Tangerine, ForkCondition::Block(2463000)),
        (Hardfork::SpuriousDragon, ForkCondition::Block(2675000)),
        (Hardfork::Byzantium, ForkCondition::Block(4370000)),
        (Hardfork::Constantinople, ForkCondition::Block(7280000)),
        (Hardfork::Petersburg, ForkCondition::Block(7280000)),
        (Hardfork::Istanbul, ForkCondition::Block(9069000)),
        (Hardfork::MuirGlacier, ForkCondition::Block(9200000)),
        (Hardfork::Berlin, ForkCondition::Block(12244000)),
        (Hardfork::London, ForkCondition::Block(12965000)),
        (Hardfork::ArrowGlacier, ForkCondition::Block(13773000)),
        (Hardfork::GrayGlacier, ForkCondition::Block(15050000)),
        (
            Hardfork::Paris,
            ForkCondition::TTD {
                fork_block: None,
                total_difficulty: U256::from(58_750_000_000_000_000_000_000_u128),
            },
        ),
        (Hardfork::Shanghai, ForkCondition::Timestamp(1681338455)),
        (Hardfork::Cancun, ForkCondition::Timestamp(1710338135)),
    ])
});

/// Ethereum Goerli hardforks
pub static GOERLI_HARDFORKS: Lazy<BTreeMap<Hardfork, ForkCondition>> = Lazy::new(|| {
    BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Homestead, ForkCondition::Block(0)),
        (Hardfork::Dao, ForkCondition::Block(0)),
        (Hardfork::Tangerine, ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
        (Hardfork::Byzantium, ForkCondition::Block(0)),
        (Hardfork::Constantinople, ForkCondition::Block(0)),
        (Hardfork::Petersburg, ForkCondition::Block(0)),
        (Hardfork::Istanbul, ForkCondition::Block(1561651)),
        (Hardfork::Berlin, ForkCondition::Block(4460644)),
        (Hardfork::London, ForkCondition::Block(5062605)),
        (
            Hardfork::Paris,
            ForkCondition::TTD { fork_block: None, total_difficulty: U256::from(10_790_000) },
        ),
        (Hardfork::Shanghai, ForkCondition::Timestamp(1678832736)),
        (Hardfork::Cancun, ForkCondition::Timestamp(1705473120)),
    ])
});

/// Ethereum Sepolia hardforks
pub static SEPOLIA_HARDFORKS: Lazy<BTreeMap<Hardfork, ForkCondition>> = Lazy::new(|| {
    BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Homestead, ForkCondition::Block(0)),
        (Hardfork::Dao, ForkCondition::Block(0)),
        (Hardfork::Tangerine, ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
        (Hardfork::Byzantium, ForkCondition::Block(0)),
        (Hardfork::Constantinople, ForkCondition::Block(0)),
        (Hardfork::Petersburg, ForkCondition::Block(0)),
        (Hardfork::Istanbul, ForkCondition::Block(0)),
        (Hardfork::MuirGlacier, ForkCondition::Block(0)),
        (Hardfork::Berlin, ForkCondition::Block(0)),
        (Hardfork::London, ForkCondition::Block(0)),
        (
            Hardfork::Paris,
            ForkCondition::TTD {
                fork_block: Some(1735371),
                total_difficulty: U256::from(17_000_000_000_000_000u64),
            },
        ),
        (Hardfork::Shanghai, ForkCondition::Timestamp(1677557088)),
        (Hardfork::Cancun, ForkCondition::Timestamp(1706655072)),
    ])
});

/// Ethereum Holesky hardforks
pub static HOLESKY_HARDFORKS: Lazy<BTreeMap<Hardfork, ForkCondition>> = Lazy::new(|| {
    BTreeMap::from([
        (Hardfork::Frontier, ForkCondition::Block(0)),
        (Hardfork::Homestead, ForkCondition::Block(0)),
        (Hardfork::Dao, ForkCondition::Block(0)),
        (Hardfork::Tangerine, ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
        (Hardfork::Byzantium, ForkCondition::Block(0)),
        (Hardfork::Constantinople, ForkCondition::Block(0)),
        (Hardfork::Petersburg, ForkCondition::Block(0)),
        (Hardfork::Istanbul, ForkCondition::Block(0)),
        (Hardfork::MuirGlacier, ForkCondition::Block(0)),
        (Hardfork::Berlin, ForkCondition::Block(0)),
        (Hardfork::London, ForkCondition::Block(0)),
        (Hardfork::Paris, ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO }),
        (Hardfork::Shanghai, ForkCondition::Timestamp(1696000704)),
        (Hardfork::Cancun, ForkCondition::Timestamp(1707305664)),
    ])
});
