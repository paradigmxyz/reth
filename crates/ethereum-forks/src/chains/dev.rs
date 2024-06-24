use crate::{ForkCondition, EthereumHardfork, ChainHardforks};
use alloy_primitives::uint;
use once_cell::sync::Lazy;

/// Dev hardforks
pub static DEV_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks(vec![
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
