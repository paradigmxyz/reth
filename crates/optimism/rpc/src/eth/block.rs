//! Loads and formats OP block RPC response.   

use reth_primitives::TransactionMeta;
use reth_provider::{BlockReaderIdExt, HeaderProvider};
use reth_rpc_eth_api::helpers::{EthApiSpec, EthBlocks, LoadBlock, LoadReceipt, LoadTransaction};
use reth_rpc_eth_types::{EthResult, EthStateCache, ReceiptBuilder};
use reth_rpc_types::{AnyTransactionReceipt, BlockId};

use crate::{op_receipt_fields, OpEthApi};

impl<Eth> EthBlocks for OpEthApi<Eth>
where
    Eth: EthBlocks + EthApiSpec + LoadTransaction,
{
    fn provider(&self) -> impl HeaderProvider {
        EthBlocks::provider(&self.inner)
    }

    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> EthResult<Option<Vec<AnyTransactionReceipt>>>
    where
        Self: LoadReceipt,
    {
        if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
            let block_number = block.number;
            let base_fee = block.base_fee_per_gas;
            let block_hash = block.hash();
            let excess_blob_gas = block.excess_blob_gas;
            let timestamp = block.timestamp;
            let block = block.unseal();

            let l1_block_info = reth_evm_optimism::extract_l1_info(&block).ok();

            let receipts = block
                .body
                .into_iter()
                .zip(receipts.iter())
                .enumerate()
                .map(|(idx, (ref tx, receipt))| {
                    let meta = TransactionMeta {
                        tx_hash: tx.hash,
                        index: idx as u64,
                        block_hash,
                        block_number,
                        base_fee,
                        excess_blob_gas,
                        timestamp,
                    };

                    let optimism_tx_meta =
                        self.build_op_tx_meta(tx, l1_block_info.clone(), timestamp)?;

                    ReceiptBuilder::new(tx, meta, receipt, &receipts).map(|builder| {
                        op_receipt_fields(builder, tx, receipt, optimism_tx_meta).build()
                    })
                })
                .collect::<EthResult<Vec<_>>>();
            return receipts.map(Some)
        }

        Ok(None)
    }
}

impl<Eth: LoadBlock> LoadBlock for OpEthApi<Eth> {
    fn provider(&self) -> impl BlockReaderIdExt {
        LoadBlock::provider(&self.inner)
    }

    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
