use crate::{ForkCondition, Hardfork};
use alloy_primitives::uint;

/// Dev hardforks
pub const DEV_HARDFORKS: [(Hardfork, ForkCondition); 14] = [
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
        ForkCondition::TTD { fork_block: None, total_difficulty: uint!(10_790_000_U256) },
    ),
    (Hardfork::Shanghai, ForkCondition::Timestamp(1678832736)),
    (Hardfork::Cancun, ForkCondition::Timestamp(1705473120)),
];
