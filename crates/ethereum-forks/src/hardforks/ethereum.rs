use alloy_primitives::U256;

use crate::{EthereumHardfork, ForkCondition};

/// Helper methods for Ethereum forks.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthereumHardforks: Clone {
    /// Retrieves [`ForkCondition`] by an [`EthereumHardfork`]. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition;

    /// Convenience method to check if an [`EthereumHardfork`] is active at a given timestamp.
    fn is_ethereum_fork_active_at_timestamp(&self, fork: EthereumHardfork, timestamp: u64) -> bool {
        self.ethereum_fork_activation(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if an [`EthereumHardfork`] is active at a given block number.
    fn is_ethereum_fork_active_at_block(&self, fork: EthereumHardfork, block_number: u64) -> bool {
        self.ethereum_fork_activation(fork).active_at_block(block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::Shanghai`] is active at a given
    /// timestamp.
    fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_ethereum_fork_active_at_timestamp(EthereumHardfork::Shanghai, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Cancun`] is active at a given timestamp.
    fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_ethereum_fork_active_at_timestamp(EthereumHardfork::Cancun, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Prague`] is active at a given timestamp.
    fn is_prague_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_ethereum_fork_active_at_timestamp(EthereumHardfork::Prague, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Osaka`] is active at a given timestamp.
    fn is_osaka_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.is_ethereum_fork_active_at_timestamp(EthereumHardfork::Osaka, timestamp)
    }

    /// Convenience method to check if [`EthereumHardfork::Byzantium`] is active at a given block
    /// number.
    fn is_byzantium_active_at_block(&self, block_number: u64) -> bool {
        self.is_ethereum_fork_active_at_block(EthereumHardfork::Byzantium, block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::SpuriousDragon`] is active at a given
    /// block number.
    fn is_spurious_dragon_active_at_block(&self, block_number: u64) -> bool {
        self.is_ethereum_fork_active_at_block(EthereumHardfork::SpuriousDragon, block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::Homestead`] is active at a given block
    /// number.
    fn is_homestead_active_at_block(&self, block_number: u64) -> bool {
        self.is_ethereum_fork_active_at_block(EthereumHardfork::Homestead, block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::London`] is active at a given block
    /// number.
    fn is_london_active_at_block(&self, block_number: u64) -> bool {
        self.is_ethereum_fork_active_at_block(EthereumHardfork::London, block_number)
    }

    /// Convenience method to check if [`EthereumHardfork::Constantinople`] is active at a given
    /// block number.
    fn is_constantinople_active_at_block(&self, block_number: u64) -> bool {
        self.is_ethereum_fork_active_at_block(EthereumHardfork::Constantinople, block_number)
    }

    /// The Paris hardfork (merge) is activated via block number. If we have knowledge of the block,
    /// this function will return true if the block number is greater than or equal to the Paris
    /// (merge) block.
    fn is_paris_active_at_block(&self, block_number: u64) -> Option<bool> {
        match self.ethereum_fork_activation(EthereumHardfork::Paris) {
            ForkCondition::TTD { activation_block_number, .. } => {
                Some(block_number >= activation_block_number)
            }
            ForkCondition::Block(paris_block) => Some(block_number >= paris_block),
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
