//! Loads and formats OP transaction RPC response.   

use reth_evm_optimism::RethL1BlockInfo;
use reth_primitives::TransactionSigned;
use reth_provider::{BlockIdReader, ChainSpecProvider};
use reth_rpc::eth::error::EthResult;
use revm::L1BlockInfo;

use crate::{error::OptimismEthApiError, OptimismApi};

/// L1 fee and data gas for a transaction, along with the L1 block info.
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

impl<Provider, Pool, Network, EvmConfig> OptimismApi<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockIdReader + ChainSpecProvider,
{
    /// Builds [`OptimismTxMeta`] object using the provided [`TransactionSigned`], L1 block
    /// info and block timestamp. The [`L1BlockInfo`] is used to calculate the l1 fee and l1 data
    /// gas for the transaction. If the [`L1BlockInfo`] is not provided, the meta info will be
    /// empty.
    pub fn build_op_tx_meta(
        &self,
        tx: &TransactionSigned,
        l1_block_info: Option<L1BlockInfo>,
        block_timestamp: u64,
    ) -> EthResult<OptimismTxMeta> {
        let Some(l1_block_info) = l1_block_info else { return Ok(OptimismTxMeta::default()) };

        let (l1_fee, l1_data_gas) = if !tx.is_deposit() {
            let envelope_buf = tx.envelope_encoded();

            let inner_l1_fee = l1_block_info
                .l1_tx_data_fee(
                    &self.inner.provider().chain_spec(),
                    block_timestamp,
                    &envelope_buf,
                    tx.is_deposit(),
                )
                .map_err(|_| OptimismEthApiError::L1BlockFeeError)?;
            let inner_l1_data_gas = l1_block_info
                .l1_data_gas(&self.inner.provider().chain_spec(), block_timestamp, &envelope_buf)
                .map_err(|_| OptimismEthApiError::L1BlockGasError)?;
            (
                Some(inner_l1_fee.saturating_to::<u128>()),
                Some(inner_l1_data_gas.saturating_to::<u128>()),
            )
        } else {
            (None, None)
        };

        Ok(OptimismTxMeta::new(Some(l1_block_info), l1_fee, l1_data_gas))
    }
}
