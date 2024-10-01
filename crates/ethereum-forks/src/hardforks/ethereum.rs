use alloy_primitives::U256;

use crate::{hardforks::Hardforks, EthereumHardfork, ForkCondition};

/// Helper methods for Ethereum forks.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthereumHardforks: Hardforks {
    /// Convenience method to check if [`EthereumHardfork::Shanghai`] is active at a given
    /// timestamp.
    fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Shanghai, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Cancun`] is active at a given timestamp.
    fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Cancun, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Prague`] is active at a given timestamp.
    fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_fork_active_at_timestamp(EthereumHardfork::Prague, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Byzantium`] is active at a given block
    /// number.
    fn is_byzantium_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::Byzantium).active_at_block(block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::SpuriousDragon`] is active at a given
    /// block number.
    fn is_spurious_dragon_active_at_block(&self, block_number: u64) -> bool {
        self.fork(EthereumHardfork::SpuriousDragon).active_at_block(block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::Homestead`] is active at a given block
    /// number.
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

    /// Returns the final total difficulty if the Paris hardfork is known.
    fn get_final_paris_total_difficulty(&self) -> Option<U256>;

    /// Returns the final total difficulty if the given block number is after the Paris hardfork.
    ///
    /// Note: technically this would also be valid for the block before the paris upgrade, but this
    /// edge case is omitted here.
    fn final_paris_total_difficulty(&self, block_number: u64) -> Option<U256>;
}
