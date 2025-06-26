use super::spec_id_at_timestamp_and_number;
use reth_evm::block::BlockExecutionError;
use revm_primitives::U256;
use revm_scroll::l1block::L1BlockInfo;
use scroll_alloy_hardforks::ScrollHardforks;

/// An extension trait for [`L1BlockInfo`] that allows us to calculate the L1 cost of a transaction
/// based off of the chain spec's activated hardfork.
pub trait RethL1BlockInfo {
    /// Forwards an L1 transaction calculation to revm and returns the gas cost.
    ///
    /// ### Takes
    /// - `chain_spec`: The chain spec for the node.
    /// - `timestamp`: The timestamp of the current block.
    /// - `block`: The block number of the current block.
    /// - `input`: The calldata of the transaction.
    /// - `is_l1_message`: Whether or not the transaction is a l1 message.
    fn l1_tx_data_fee(
        &mut self,
        chain_spec: impl ScrollHardforks,
        timestamp: u64,
        block: u64,
        input: &[u8],
        compression_ratio: Option<U256>,
        is_l1_message: bool,
    ) -> Result<U256, BlockExecutionError>;
}

impl RethL1BlockInfo for L1BlockInfo {
    fn l1_tx_data_fee(
        &mut self,
        chain_spec: impl ScrollHardforks,
        timestamp: u64,
        block_number: u64,
        input: &[u8],
        compression_ratio: Option<U256>,
        is_l1_message: bool,
    ) -> Result<U256, BlockExecutionError> {
        if is_l1_message {
            return Ok(U256::ZERO);
        }

        let spec_id = spec_id_at_timestamp_and_number(timestamp, block_number, chain_spec);
        Ok(self.calculate_tx_l1_cost(input, spec_id, compression_ratio))
    }
}
