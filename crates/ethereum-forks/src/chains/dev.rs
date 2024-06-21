use crate::{EthereumForks, ForkCondition, Hardfork, HardforkTrait};
use alloy_primitives::uint;
use once_cell::sync::Lazy;

/// Dev hardforks
pub const DEV_HARDFORKS: Lazy<EthereumForks> = Lazy::new(|| {
    EthereumForks(vec![
        (Hardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (Hardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (Hardfork::Dao.boxed(), ForkCondition::Block(0)),
        (Hardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (Hardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (Hardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (Hardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (Hardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (Hardfork::Istanbul.boxed(), ForkCondition::Block(1561651)),
        (Hardfork::Berlin.boxed(), ForkCondition::Block(4460644)),
        (Hardfork::London.boxed(), ForkCondition::Block(5062605)),
        (
            Hardfork::Paris.boxed(),
            ForkCondition::TTD { fork_block: None, total_difficulty: uint!(10_790_000_U256) },
        ),
        (Hardfork::Shanghai.boxed(), ForkCondition::Timestamp(1678832736)),
        (Hardfork::Cancun.boxed(), ForkCondition::Timestamp(1705473120)),
    ])
});
