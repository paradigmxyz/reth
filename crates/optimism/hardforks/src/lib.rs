//! OP-Reth hard forks.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

extern crate alloc;

pub mod hardfork;

mod dev;

pub use dev::DEV_HARDFORKS;
pub use hardfork::OptimismHardfork;

use reth_ethereum_forks::EthereumHardforks;

/// Extends [`EthereumHardforks`] with optimism helper methods.
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
