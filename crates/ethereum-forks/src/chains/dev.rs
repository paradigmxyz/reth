use crate::{EthereumForks, ForkCondition, Hardfork, HardforkTrait};
use alloy_primitives::uint;
use once_cell::sync::Lazy;

/// Dev hardforks
pub const DEV_HARDFORKS: Lazy<EthereumForks> = Lazy::new(|| {
    EthereumForks(vec![
        (Box::new(Hardfork::Frontier), ForkCondition::Block(0)),
        (Box::new(Hardfork::Homestead), ForkCondition::Block(0)),
        (Box::new(Hardfork::Dao), ForkCondition::Block(0)),
        (Box::new(Hardfork::Tangerine), ForkCondition::Block(0)),
        (Box::new(Hardfork::SpuriousDragon), ForkCondition::Block(0)),
        (Box::new(Hardfork::Byzantium), ForkCondition::Block(0)),
        (Box::new(Hardfork::Constantinople), ForkCondition::Block(0)),
        (Box::new(Hardfork::Petersburg), ForkCondition::Block(0)),
        (Box::new(Hardfork::Istanbul), ForkCondition::Block(1561651)),
        (Box::new(Hardfork::Berlin), ForkCondition::Block(4460644)),
        (Box::new(Hardfork::London), ForkCondition::Block(5062605)),
        (
            Box::new(Hardfork::Paris),
            ForkCondition::TTD { fork_block: None, total_difficulty: uint!(10_790_000_U256) },
        ),
        (Box::new(Hardfork::Shanghai), ForkCondition::Timestamp(1678832736)),
        (Box::new(Hardfork::Cancun), ForkCondition::Timestamp(1705473120)),
    ])
});
