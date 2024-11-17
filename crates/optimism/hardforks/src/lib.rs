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
pub use hardfork::OpHardfork;

use reth_ethereum_forks::EthereumHardforks;

/// Extends [`EthereumHardforks`] with optimism helper methods.
pub trait OpHardforks: EthereumHardforks {
    /// Convenience method to check if [`OpHardfork::Bedrock`] is active at a given block
    /// number.
    fn is_bedrock_active_at_block(&self, block_number: u64) -> bool {
        self.fork(OpHardfork::Bedrock).active_at_block(block_number)
    }

    /// Returns `true` if [`Canyon`](OpHardfork::Canyon) is active at given block timestamp.
    fn is_canyon_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OpHardfork::Canyon).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Ecotone`](OpHardfork::Ecotone) is active at given block timestamp.
    fn is_ecotone_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OpHardfork::Ecotone).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Fjord`](OpHardfork::Fjord) is active at given block timestamp.
    fn is_fjord_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OpHardfork::Fjord).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Granite`](OpHardfork::Granite) is active at given block timestamp.
    fn is_granite_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OpHardfork::Granite).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Holocene`](OpHardfork::Holocene) is active at given block
    /// timestamp.
    fn is_holocene_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OpHardfork::Holocene).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Regolith`](OpHardfork::Regolith) is active at given block
    /// timestamp.
    fn is_regolith_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork(OpHardfork::Regolith).active_at_timestamp(timestamp)
    }
}
