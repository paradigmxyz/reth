use alloc::vec::Vec;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{logs_bloom, Bloom};
use reth_primitives_traits::Receipt;
use revm::db::BundleState;

/// The result of executing a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockExecutionResult<T> {
    /// All the receipts of the transactions in the block.
    pub receipts: Vec<T>,
    /// All the EIP-7685 requests in the block.
    pub requests: Requests,
    /// The total gas used by the block.
    pub gas_used: u64,
}

impl<T: Receipt> BlockExecutionResult<T> {
    /// Calculates the logs bloom.
    pub fn logs_bloom(&self) -> Bloom {
        logs_bloom(self.receipts.iter().flat_map(|r| r.logs()))
    }
}

/// [`BlockExecutionResult`] combined with state.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::AsRef,
    derive_more::AsMut,
    derive_more::Deref,
    derive_more::DerefMut,
)]
pub struct BlockExecutionOutput<T> {
    /// All the receipts of the transactions in the block.
    #[as_ref]
    #[as_mut]
    #[deref]
    #[deref_mut]
    pub result: BlockExecutionResult<T>,
    /// The changed state of the block after execution.
    pub state: BundleState,
}
