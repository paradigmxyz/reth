//! Formats OP transaction RPC response.   

use revm::L1BlockInfo;

/// Optimism Transaction Metadata
///
/// Includes the L1 fee and data gas for the tx along with the L1
/// block info. In order to pass the [OptimismTxMeta] into the
/// async colored [ReceiptBuilder], a reference counter
/// for the L1 block info is used so the L1 block info can be
/// shared between receipts.
#[derive(Debug, Default, Clone)]
pub struct OptimismTxMeta {
    /// The L1 block info.
    pub l1_block_info: Option<L1BlockInfo>,
    /// The L1 fee for the block.
    pub l1_fee: Option<u128>,
    /// The L1 data gas for the block.
    pub l1_data_gas: Option<u128>,
}

impl OptimismTxMeta {
    /// Creates a new [OptimismTxMeta].
    pub fn new(
        l1_block_info: Option<L1BlockInfo>,
        l1_fee: Option<u128>,
        l1_data_gas: Option<u128>,
    ) -> Self {
        Self { l1_block_info, l1_fee, l1_data_gas }
    }
}
