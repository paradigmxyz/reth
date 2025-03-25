//! Hard forks of bsc protocol.
use bsc::BscHardfork;
use reth_chainspec::{EthereumHardforks, ForkCondition};

pub mod bsc;

/// Extends [`EthereumHardforks`] with bsc helper methods.
pub trait BscHardforks: EthereumHardforks {
    /// Retrieves [`ForkCondition`] by an [`OpHardfork`]. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn bsc_fork_activation(&self, fork: BscHardfork) -> ForkCondition;

    /// Convenience method to check if [`BscHardfork::Ramanujan`] is firstly active at a given
    /// block.
    fn is_on_ramanujan_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Ramanujan).transitions_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Ramanujan`] is active at a given block.
    fn is_ramanujan_active_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Ramanujan).active_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Euler`] is firstly active at a given block.
    fn is_on_euler_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Euler).transitions_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Euler`] is active at a given block.
    fn is_euler_active_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Euler).active_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Planck`] is firstly active at a given block.
    fn is_on_planck_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Planck).transitions_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Planck`] is active at a given block.
    fn is_planck_active_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Planck).active_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Luban`] is firstly active at a given block.
    fn is_on_luban_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Luban).transitions_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Luban`] is active at a given block.
    fn is_luban_active_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Luban).active_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Plato`] is firstly active at a given block.
    fn is_on_plato_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Plato).transitions_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Plato`] is active at a given block.
    fn is_plato_active_at_block(&self, block_number: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Plato).active_at_block(block_number)
    }

    /// Convenience method to check if [`BscHardfork::Kepler`] is firstly active at a given
    /// timestamp and parent timestamp.
    fn is_on_kepler_at_timestamp(&self, timestamp: u64, parent_timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Kepler)
            .transitions_at_timestamp(timestamp, parent_timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Kepler`] is active at a given timestamp.
    fn is_kepler_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Kepler).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Feynman`] is firstly active at a given
    /// timestamp and parent timestamp.
    fn is_on_feynman_at_timestamp(&self, timestamp: u64, parent_timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Feynman)
            .transitions_at_timestamp(timestamp, parent_timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Feynman`] is active at a given timestamp.
    fn is_feynman_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Feynman).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [`BscHardfork::FeynmanFix`] is firstly active at a given
    /// timestamp and parent timestamp.
    fn is_on_feynman_fix_at_timestamp(&self, timestamp: u64, parent_timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::FeynmanFix)
            .transitions_at_timestamp(timestamp, parent_timestamp)
    }

    /// Convenience method to check if [`BscHardfork::FeynmanFix`] is active at a given timestamp.
    fn is_feynman_fix_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::FeynmanFix).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Haber`] is firstly active at a given timestamp
    /// and parent timestamp.
    fn is_on_haber_at_timestamp(&self, timestamp: u64, parent_timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Haber)
            .transitions_at_timestamp(timestamp, parent_timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Haber`] is active at a given timestamp.
    fn is_haber_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Haber).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [`BscHardfork::HaberFix`] is firstly active at a given
    /// timestamp and parent timestamp.
    fn is_on_haber_fix_at_timestamp(&self, timestamp: u64, parent_timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::HaberFix)
            .transitions_at_timestamp(timestamp, parent_timestamp)
    }

    /// Convenience method to check if [`BscHardfork::HaberFix`] is active at a given timestamp.
    fn is_haber_fix_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::HaberFix).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Bohr`] is firstly active at a given
    /// timestamp and parent timestamp.
    fn is_on_bohr_at_timestamp(&self, timestamp: u64, parent_timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Bohr)
            .transitions_at_timestamp(timestamp, parent_timestamp)
    }

    /// Convenience method to check if [`BscHardfork::Bohr`] is active at a given timestamp.
    fn is_bohr_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.bsc_fork_activation(BscHardfork::Bohr).active_at_timestamp(timestamp)
    }
}
