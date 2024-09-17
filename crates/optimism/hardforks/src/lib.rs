extern crate alloc;

pub mod hardfork;

mod dev;

pub use dev::DEV_HARDFORKS;
pub use hardfork::OptimismHardfork;

use reth_ethereum_forks::{ChainHardforks, EthereumHardforks};

/// Extends [`crate::EthereumHardforks`] with optimism helper methods.
pub trait OptimismHardforks: EthereumHardforks {
    /// Convenience method to check if [`OptimismHardfork::Bedrock`] is active at a given block
    /// number.
    fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(OptimismHardfork::Bedrock).active_at_block(block_number)
    }

    /// Returns `true` if [`Ecotone`](OptimismHardfork::Ecotone) is active at given block timestamp.
    fn is_ecotone_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OptimismHardfork::Ecotone).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Ecotone`](OptimismHardfork::Ecotone) is active at given block timestamp.
    fn is_fjord_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OptimismHardfork::Ecotone).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Granite`](OptimismHardfork::Granite) is active at given block timestamp.
    fn is_granite_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OptimismHardfork::Granite).active_at_timestamp(timestamp)
    }
}

impl OptimismHardforks for ChainHardforks {}
