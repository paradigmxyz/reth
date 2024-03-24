use alloy_primitives::Bloom;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;

use reth_primitives::{Address, BlockId, BlockNumberOrTag, TxHash, B256};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_types::{
    Block, BlockDetails, BlockTransactions, ContractCreator, InternalOperation,
    OtsBlockTransactions, OtsTransactionReceipt, TraceEntry, Transaction, TransactionsWithReceipts,
};

use crate::result::internal_rpc_err;

const API_LEVEL: u64 = 8;

/// Otterscan API.
#[derive(Debug)]
pub struct OtterscanApi<Eth> {
    eth: Eth,
}

impl<Eth> OtterscanApi<Eth> {
    /// Creates a new instance of `Otterscan`.
    pub fn new(eth: Eth) -> Self {
        Self { eth }
    }
}

#[async_trait]
impl<Eth> OtterscanServer for OtterscanApi<Eth>
where
    Eth: EthApiServer,
{
    /// Handler for `ots_hasCode`
    async fn has_code(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<bool> {
        self.eth.get_code(address, block_number).await.map(|code| !code.is_empty())
    }

    /// Handler for `ots_getApiLevel`
    async fn get_api_level(&self) -> RpcResult<u64> {
        Ok(API_LEVEL)
    }

    /// Handler for `ots_getInternalOperations`
    async fn get_internal_operations(&self, _tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_getTransactionError`
    async fn get_transaction_error(&self, _tx_hash: TxHash) -> RpcResult<String> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_traceTransaction`
    async fn trace_transaction(&self, _tx_hash: TxHash) -> RpcResult<TraceEntry> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_getBlockDetails`
    async fn get_block_details(
        &self,
        block_number: BlockNumberOrTag,
    ) -> RpcResult<Option<BlockDetails>> {
        let block = self.eth.block_by_number(block_number, true).await?;
        Ok(block.map(Into::into))
    }

    /// Handler for `getBlockDetailsByHash`
    async fn get_block_details_by_hash(&self, block_hash: B256) -> RpcResult<Option<BlockDetails>> {
        let block = self.eth.block_by_hash(block_hash, true).await?;
        Ok(block.map(Into::into))
    }

    /// Handler for `getBlockTransactions`
    async fn get_block_transactions(
        &self,
        block_number: BlockNumberOrTag,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<OtsBlockTransactions> {
        let block = self
            .eth
            .block_by_number(block_number, true)
            .await?
            .ok_or_else(|| internal_rpc_err("block not found"))?;

        if let BlockTransactions::Full(transactions) = &block.transactions {
            let mut transactions = transactions.clone();

            // The input field returns only the 4 bytes method selector instead of the entire
            // calldata byte blob.
            for tx in &mut transactions {
                tx.input = tx.input.slice(..4);
            }

            let mut receipts =
                self.eth.block_receipts(BlockId::Number(block_number)).await?.unwrap_or(vec![]);

            // For receipts
            //  1. logs attribute returns null
            //  2. logsBloom attribute returns null.
            receipts.iter_mut().for_each(|receipt| {
                receipt.logs = vec![];
                receipt.logs_bloom = Bloom::default();
            });

            // Crop page
            let page_end = transactions.len().saturating_sub(page_number * page_size);
            let page_start = page_end.saturating_sub(page_size);

            // Crop transactions
            transactions = transactions.drain(page_start..page_end).collect();

            let block =
                Block { transactions: BlockTransactions::Full(transactions), ..block.inner };

            // Crop receipts and transform them into OtsTransactionReceipt
            let timestamp = u64::try_from(block.header.timestamp).unwrap_or(0);
            let receipts = receipts
                .drain(page_start..page_end)
                .map(|receipt| OtsTransactionReceipt { receipt, timestamp })
                .collect();
            Ok(OtsBlockTransactions { fullblock: block.into(), receipts })
        } else {
            return Err(internal_rpc_err("block is not full"));
        }
    }

    /// Handler for `searchTransactionsBefore`
    async fn search_transactions_before(
        &self,
        _address: Address,
        _block_number: BlockNumberOrTag,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `searchTransactionsAfter`
    async fn search_transactions_after(
        &self,
        _address: Address,
        _block_number: BlockNumberOrTag,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getTransactionBySenderAndNonce`
    async fn get_transaction_by_sender_and_nonce(
        &self,
        _sender: Address,
        _nonce: u64,
    ) -> RpcResult<Option<Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getContractCreator`
    async fn get_contract_creator(&self, _address: Address) -> RpcResult<Option<ContractCreator>> {
        Err(internal_rpc_err("unimplemented"))
    }
}
