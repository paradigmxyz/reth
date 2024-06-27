use crate::{ChainHardforks, EthereumHardforks, OptimismHardfork};

/// Extends [`crate::EthereumHardforks`] with optimism helper methods.
pub trait OptimismHardforks: EthereumHardforks {
    /// Convenience method to check if [`OptimismHardfork::Bedrock`] is active at a given block
    /// number.
    fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(OptimismHardfork::Bedrock).active_at_block(block_number)
    }
}

impl OptimismHardforks for ChainHardforks {}
