use alloy_primitives::{B256, U256};
use reth_primitives::{Receipt, Request};
use revm::db::BundleState;

/// A helper type for ethereum block inputs that consists of a block and the total difficulty.
#[derive(Debug)]
pub struct BlockExecutionInput<'a, Block> {
    /// The block to execute.
    pub block: &'a Block,
    /// The total difficulty of the block.
    pub total_difficulty: U256,
}

impl<'a, Block> BlockExecutionInput<'a, Block> {
    /// Creates a new input.
    pub const fn new(block: &'a Block, total_difficulty: U256) -> Self {
        Self { block, total_difficulty }
    }
}

impl<'a, Block> From<(&'a Block, U256)> for BlockExecutionInput<'a, Block> {
    fn from((block, total_difficulty): (&'a Block, U256)) -> Self {
        Self::new(block, total_difficulty)
    }
}

/// The output of an ethereum block.
///
/// Contains the state changes, transaction receipts, and total gas used in the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockExecutionOutput<T> {
    /// The changed state of the block after execution.
    pub state: BundleState,
    /// All the receipts of the transactions in the block.
    pub receipts: Vec<T>,
    /// All the EIP-7685 requests of the transactions in the block.
    pub requests: Vec<Request>,
    /// The total gas used by the block.
    pub gas_used: u64,
}

pub trait BlockExecOutput {
    type Receipt;

    fn state(&self) -> &BundleState;
    fn receipts(&self) -> &[Self::Receipt];
    fn requests(&self) -> &[Request];
    fn gas_used(&self) -> u64;
    /// Calculates the receipts root of the block.
    fn receipts_root_slow(&self) -> Option<B256>;
}

/// The output of an ethereum block.
///
/// Contains the state changes, transaction receipts, and total gas used in the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthBlockExecutionOutput {
    /// The changed state of the block after execution.
    pub state: BundleState,
    /// All the receipts of the transactions in the block.
    pub receipts: Vec<Receipt>,
    /// All the EIP-7685 requests of the transactions in the block.
    pub requests: Vec<Request>,
    /// The total gas used by the block.
    pub gas_used: u64,
}

impl BlockExecOutput for EthBlockExecutionOutput {
    type Receipt = Receipt;

    fn state(&self) -> &BundleState {
        &self.state
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.receipts
    }

    fn requests(&self) -> &[Request] {
        &self.requests
    }

    fn gas_used(&self) -> u64 {
        self.gas_used
    }

    /// Returns the receipt root for all recorded receipts.
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is a expensive operation.
    fn receipts_root_slow(&self) -> Option<B256> {
        Some(reth_primitives::proofs::calculate_receipt_root_no_memo(&self.receipts))
    }
}
