//! Ethereum execution helpers.

use alloc::borrow::Cow;
use alloy_consensus::Header;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Address, Bytes, B256};

pub use alloy_evm::eth::{dao_fork, eip6110, spec};

/// Context for Ethereum block execution.
#[derive(Debug, Clone)]
pub struct EthBlockExecutionCtx<'a> {
    /// Parent block hash.
    pub parent_hash: B256,
    /// Parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// Block ommers.
    pub ommers: &'a [Header],
    /// Block withdrawals.
    pub withdrawals: Option<Cow<'a, [Withdrawal]>>,
    /// Block extra data.
    pub extra_data: Bytes,
    /// Block transactions count hint. Used to preallocate the receipts vector.
    pub tx_count_hint: Option<usize>,
    /// Slot number (EIP-7843, Amsterdam).
    pub slot_number: Option<u64>,
}

/// Additional attributes required to configure the next block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextEvmEnvAttributes {
    /// The timestamp of the next block.
    pub timestamp: u64,
    /// The suggested fee recipient for the next block.
    pub suggested_fee_recipient: Address,
    /// The randomness value for the next block.
    pub prev_randao: B256,
    /// Block gas limit.
    pub gas_limit: u64,
    /// Slot number (EIP-7843).
    pub slot_number: Option<u64>,
}
