mod sealed;
pub use sealed::SealedHeader;

mod error;
pub use error::HeaderError;

#[cfg(any(test, feature = "test-utils", feature = "arbitrary"))]
pub mod test_utils;

pub use alloy_consensus::Header;

use alloy_primitives::{Address, BlockNumber, B256, U256};

/// Bincode-compatible header type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::sealed::serde_bincode_compat::SealedHeader;
}

/// Trait for extracting specific Ethereum block data from a header
pub trait BlockHeader {
    /// Retrieves the beneficiary (miner) of the block
    fn beneficiary(&self) -> Address;

    /// Retrieves the difficulty of the block
    fn difficulty(&self) -> U256;

    /// Retrieves the block number
    fn number(&self) -> BlockNumber;

    /// Retrieves the gas limit of the block
    fn gas_limit(&self) -> u64;

    /// Retrieves the timestamp of the block
    fn timestamp(&self) -> u64;

    /// Retrieves the mix hash of the block
    fn mix_hash(&self) -> B256;

    /// Retrieves the base fee per gas of the block, if available
    fn base_fee_per_gas(&self) -> Option<u64>;

    /// Retrieves the excess blob gas of the block, if available
    fn excess_blob_gas(&self) -> Option<u64>;
}

impl BlockHeader for Header {
    fn beneficiary(&self) -> Address {
        self.beneficiary
    }

    fn difficulty(&self) -> U256 {
        self.difficulty
    }

    fn number(&self) -> BlockNumber {
        self.number
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn mix_hash(&self) -> B256 {
        self.mix_hash
    }

    fn base_fee_per_gas(&self) -> Option<u64> {
        self.base_fee_per_gas
    }

    fn excess_blob_gas(&self) -> Option<u64> {
        self.excess_blob_gas
    }
}
