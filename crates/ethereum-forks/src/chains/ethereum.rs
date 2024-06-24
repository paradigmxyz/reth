use crate::{
    hardforks::{ChainHardforks, Hardforks},
    EthereumHardfork, ForkCondition,
};
use alloy_primitives::{uint, U256};
use once_cell::sync::Lazy;

/// Helper methods for Ethereum forks.
pub trait EthereumHardforks: Hardforks {
    /// Convenience method to check if [`Hardfork::Shanghai`] is active at a given timestamp.
    fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Shanghai, timestamp)
    }

    /// Convenience method to check if [`Hardfork::Cancun`] is active at a given timestamp.
    fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Cancun, timestamp)
    }

    /// Convenience method to check if [`Hardfork::Prague`] is active at a given timestamp.
    fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Prague, timestamp)
    }

    /// Convenience method to check if [`Hardfork::Byzantium`] is active at a given block number.
    fn is_byzantium_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::Byzantium).active_at_block(block_number)
    }

    /// Convenience method to check if [`Hardfork::SpuriousDragon`] is active at a given block
    /// number.
    fn is_spurious_dragon_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::SpuriousDragon).active_at_block(block_number)
    }

    /// Convenience method to check if [`Hardfork::Homestead`] is active at a given block number.
    fn is_homestead_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::Homestead).active_at_block(block_number)
    }

    /// The Paris hardfork (merge) is activated via block number. If we have knowledge of the block,
    /// this function will return true if the block number is greater than or equal to the Paris
    /// (merge) block.
    fn is_paris_active_at_block(&self, block_number: u64) -> Option<bool> {
        match self.fork(EthereumHardfork::Paris) {
            ForkCondition::Block(paris_block) => Some(block_number >= paris_block),
            ForkCondition::TTD { fork_block, .. } => {
                fork_block.map(|paris_block| block_number >= paris_block)
            }
            _ => None,
        }
    }
}

impl EthereumHardforks for ChainHardforks {}

/// Ethereum mainnet hardforks
pub static MAINNET_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks(vec![
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

/// Ethereum Sepolia hardforks
pub static SEPOLIA_HARDFORKS: Lazy<ChainHardforks> = Lazy::new(|| {
    ChainHardforks(vec![
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
    ChainHardforks(vec![
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
