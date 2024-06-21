use crate::{
    generate_forks_type,
    hardforks::{HardforksBaseType, HardforksTrait},
    ForkCondition, Hardfork, HardforkTrait,
};
use alloy_primitives::{uint, U256};
use once_cell::sync::Lazy;

generate_forks_type!(
    /// Wrapper type over a list of Ethereum forks.
    EthereumForks
);

impl EthereumHardforksTrait for EthereumForks {}

/// Helper methods for Ethereum forks.
pub trait EthereumHardforksTrait: HardforksTrait {
    /// Convenience method to check if [`Hardfork::Shanghai`] is active at a given timestamp.
    fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(Hardfork::Shanghai, timestamp)
    }

    /// Convenience method to check if [`Hardfork::Cancun`] is active at a given timestamp.
    fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(Hardfork::Cancun, timestamp)
    }

    /// Convenience method to check if [`Hardfork::Prague`] is active at a given timestamp.
    fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(Hardfork::Prague, timestamp)
    }

    /// Convenience method to check if [`Hardfork::Byzantium`] is active at a given block number.
    fn is_byzantium_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::Byzantium).active_at_block(block_number)
    }

    /// Convenience method to check if [`Hardfork::SpuriousDragon`] is active at a given block
    /// number.
    fn is_spurious_dragon_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::SpuriousDragon).active_at_block(block_number)
    }

    /// Convenience method to check if [`Hardfork::Homestead`] is active at a given block number.
    fn is_homestead_active_at_block(&self, block_number: u64) -> bool {
        self.fork(Hardfork::Homestead).active_at_block(block_number)
    }

    /// The Paris hardfork (merge) is activated via block number. If we have knowledge of the block,
    /// this function will return true if the block number is greater than or equal to the Paris
    /// (merge) block.
    fn is_paris_active_at_block(&self, block_number: u64) -> Option<bool> {
        match self.fork(Hardfork::Paris) {
            ForkCondition::Block(paris_block) => Some(block_number >= paris_block),
            ForkCondition::TTD { fork_block, .. } => {
                fork_block.map(|paris_block| block_number >= paris_block)
            }
            _ => None,
        }
    }
}

/// Ethereum mainnet hardforks
pub const MAINNET_HARDFORKS: Lazy<EthereumForks> = Lazy::new(|| {
    EthereumForks(vec![
        (Box::new(Hardfork::Frontier), ForkCondition::Block(0)),
        (Box::new(Hardfork::Homestead), ForkCondition::Block(1150000)),
        (Box::new(Hardfork::Dao), ForkCondition::Block(1920000)),
        (Box::new(Hardfork::Tangerine), ForkCondition::Block(2463000)),
        (Box::new(Hardfork::SpuriousDragon), ForkCondition::Block(2675000)),
        (Box::new(Hardfork::Byzantium), ForkCondition::Block(4370000)),
        (Box::new(Hardfork::Constantinople), ForkCondition::Block(7280000)),
        (Box::new(Hardfork::Petersburg), ForkCondition::Block(7280000)),
        (Box::new(Hardfork::Istanbul), ForkCondition::Block(9069000)),
        (Box::new(Hardfork::MuirGlacier), ForkCondition::Block(9200000)),
        (Box::new(Hardfork::Berlin), ForkCondition::Block(12244000)),
        (Box::new(Hardfork::London), ForkCondition::Block(12965000)),
        (Box::new(Hardfork::ArrowGlacier), ForkCondition::Block(13773000)),
        (Box::new(Hardfork::GrayGlacier), ForkCondition::Block(15050000)),
        (
            Box::new(Hardfork::Paris),
            ForkCondition::TTD {
                fork_block: None,
                total_difficulty: uint!(58_750_000_000_000_000_000_000_U256),
            },
        ),
        (Box::new(Hardfork::Shanghai), ForkCondition::Timestamp(1681338455)),
        (Box::new(Hardfork::Cancun), ForkCondition::Timestamp(1710338135)),
    ])
});

/// Ethereum Goerli hardforks
pub static GOERLI_HARDFORKS: Lazy<EthereumForks> = Lazy::new(|| {
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

/// Ethereum Sepolia hardforks
pub static SEPOLIA_HARDFORKS: Lazy<EthereumForks> = Lazy::new(|| {
    EthereumForks(vec![
        (Box::new(Hardfork::Frontier), ForkCondition::Block(0)),
        (Box::new(Hardfork::Homestead), ForkCondition::Block(0)),
        (Box::new(Hardfork::Dao), ForkCondition::Block(0)),
        (Box::new(Hardfork::Tangerine), ForkCondition::Block(0)),
        (Box::new(Hardfork::SpuriousDragon), ForkCondition::Block(0)),
        (Box::new(Hardfork::Byzantium), ForkCondition::Block(0)),
        (Box::new(Hardfork::Constantinople), ForkCondition::Block(0)),
        (Box::new(Hardfork::Petersburg), ForkCondition::Block(0)),
        (Box::new(Hardfork::Istanbul), ForkCondition::Block(0)),
        (Box::new(Hardfork::MuirGlacier), ForkCondition::Block(0)),
        (Box::new(Hardfork::Berlin), ForkCondition::Block(0)),
        (Box::new(Hardfork::London), ForkCondition::Block(0)),
        (
            Box::new(Hardfork::Paris),
            ForkCondition::TTD {
                fork_block: Some(1735371),
                total_difficulty: uint!(17_000_000_000_000_000_U256),
            },
        ),
        (Box::new(Hardfork::Shanghai), ForkCondition::Timestamp(1677557088)),
        (Box::new(Hardfork::Cancun), ForkCondition::Timestamp(1706655072)),
    ])
});

/// Ethereum Holesky hardforks
pub const HOLESKY_HARDFORKS: Lazy<EthereumForks> = Lazy::new(|| {
    EthereumForks(vec![
        (Box::new(Hardfork::Frontier), ForkCondition::Block(0)),
        (Box::new(Hardfork::Homestead), ForkCondition::Block(0)),
        (Box::new(Hardfork::Dao), ForkCondition::Block(0)),
        (Box::new(Hardfork::Tangerine), ForkCondition::Block(0)),
        (Box::new(Hardfork::SpuriousDragon), ForkCondition::Block(0)),
        (Box::new(Hardfork::Byzantium), ForkCondition::Block(0)),
        (Box::new(Hardfork::Constantinople), ForkCondition::Block(0)),
        (Box::new(Hardfork::Petersburg), ForkCondition::Block(0)),
        (Box::new(Hardfork::Istanbul), ForkCondition::Block(0)),
        (Box::new(Hardfork::MuirGlacier), ForkCondition::Block(0)),
        (Box::new(Hardfork::Berlin), ForkCondition::Block(0)),
        (Box::new(Hardfork::London), ForkCondition::Block(0)),
        (
            Box::new(Hardfork::Paris),
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        ),
        (Box::new(Hardfork::Shanghai), ForkCondition::Timestamp(1696000704)),
        (Box::new(Hardfork::Cancun), ForkCondition::Timestamp(1707305664)),
    ])
});

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn type_conversion() {
        let forks: EthereumForks = MAINNET_HARDFORKS.clone();
    }
}
