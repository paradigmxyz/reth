//! Loads and formats OP receipt RPC response.   

use reth_node_api::FullNodeComponents;
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, LoadReceipt, LoadTransaction},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, EthStateCache, ReceiptBuilder};
use reth_rpc_types::{AnyTransactionReceipt, OptimismTransactionReceiptFields};

use crate::{OpEthApi, OpEthApiError, OptimismTxMeta};

impl<N> LoadReceipt for OpEthApi<N>
where
    Self: EthApiSpec + LoadTransaction,
    Self::Error: From<OpEthApiError>,
    N: FullNodeComponents,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    async fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> Result<AnyTransactionReceipt, Self::Error> {
        let (block, receipts) = LoadReceipt::cache(self)
            .get_block_and_receipts(meta.block_hash)
            .await
            .map_err(Self::Error::from_eth_err)?
            .ok_or(Self::Error::from_eth_err(EthApiError::UnknownBlockNumber))?;

        let block = block.unseal();
        let l1_block_info = reth_evm_optimism::extract_l1_info(&block).ok();
        let optimism_tx_meta = self.build_op_tx_meta(&tx, l1_block_info, block.timestamp)?;

        let resp_builder = ReceiptBuilder::new(&tx, meta, &receipt, &receipts)
            .map_err(Self::Error::from_eth_err)?;
        let resp_builder = op_receipt_fields(resp_builder, &tx, &receipt, optimism_tx_meta);

        Ok(resp_builder.build())
    }
}

/// Applies OP specific fields to a receipt builder.
pub fn op_receipt_fields(
    resp_builder: ReceiptBuilder,
    tx: &TransactionSigned,
    receipt: &Receipt,
    optimism_tx_meta: OptimismTxMeta,
) -> ReceiptBuilder {
    let mut op_fields = OptimismTransactionReceiptFields::default();

    if tx.is_deposit() {
        op_fields.deposit_nonce = receipt.deposit_nonce;
        op_fields.deposit_receipt_version = receipt.deposit_receipt_version;
    } else if let Some(l1_block_info) = optimism_tx_meta.l1_block_info {
        op_fields.l1_fee = optimism_tx_meta.l1_fee;
        op_fields.l1_gas_used = optimism_tx_meta.l1_data_gas.map(|dg| {
            dg + l1_block_info.l1_fee_overhead.unwrap_or_default().saturating_to::<u128>()
        });
        op_fields.l1_fee_scalar = Some(f64::from(l1_block_info.l1_base_fee_scalar) / 1_000_000.0);
        op_fields.l1_gas_price = Some(l1_block_info.l1_base_fee.saturating_to());
    }

    resp_builder.add_other_fields(op_fields.into())
}
