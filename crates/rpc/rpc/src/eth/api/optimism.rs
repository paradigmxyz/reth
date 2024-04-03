//! Optimism helpers.

use reqwest::Client;
use reth_primitives::U256;
use revm::L1BlockInfo;

/// Optimism Transaction Metadata
///
/// Includes the L1 fee and data gas for the tx along with the L1
/// block info. In order to pass the [OptimismTxMeta] into the
/// async colored `build_transaction_receipt_with_block_receipts`
/// function, a reference counter for the L1 block info is
/// used so the L1 block info can be shared between receipts.
#[derive(Debug, Default, Clone)]
pub(crate) struct OptimismTxMeta {
    /// The L1 block info.
    pub(crate) l1_block_info: Option<L1BlockInfo>,
    /// The L1 fee for the block.
    pub(crate) l1_fee: Option<U256>,
    /// The L1 data gas for the block.
    pub(crate) l1_data_gas: Option<U256>,
}

impl OptimismTxMeta {
    /// Creates a new [OptimismTxMeta].
    pub(crate) fn new(
        l1_block_info: Option<L1BlockInfo>,
        l1_fee: Option<U256>,
        l1_data_gas: Option<U256>,
    ) -> Self {
        Self { l1_block_info, l1_fee, l1_data_gas }
    }
}

/// A client to interact with a Sequencer
#[derive(Debug, Default, Clone)]
pub struct SequencerClient {
    /// The endpoint of the sequencer
    pub sequencer_endpoint: String,
    /// The HTTP client
    pub http_client: Client,
}

impl SequencerClient {
    /// Creates a new [SequencerClient].
    pub fn new(sequencer_endpoint: impl Into<String>, http_client: Client) -> Self {
        Self { sequencer_endpoint: sequencer_endpoint.into(), http_client }
    }
}
