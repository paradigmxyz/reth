use alloy_primitives::Bytes;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use revm::inspectors::NoOpInspector;
use revm_inspectors::transfer::{TransferInspector, TransferKind};
use revm_primitives::ExecutionResult;

use reth_primitives::{Address, BlockId, BlockNumberOrTag, TxHash, B256};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_types::{
    BlockDetails, BlockTransactions, ContractCreator, InternalOperation, OperationType,
    OtsBlockTransactions, OtsTransactionReceipt, TraceEntry, Transaction, TransactionsWithReceipts,
};

use crate::{eth::EthTransactions, result::internal_rpc_err};

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
    Eth: EthApiServer + EthTransactions,
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
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>> {
        let internal_operations = self
            .eth
            .spawn_trace_transaction_in_block_with_inspector(
                tx_hash,
                TransferInspector::new(false),
                |_tx_info, inspector, _, _| Ok(inspector.into_transfers()),
            )
            .await?
            .map(|transfer_operations| {
                transfer_operations
                    .iter()
                    .map(|op| InternalOperation {
                        from: op.from,
                        to: op.to,
                        value: op.value,
                        r#type: match op.kind {
                            TransferKind::Call => OperationType::OpTransfer,
                            TransferKind::Create => OperationType::OpCreate,
                            TransferKind::Create2 => OperationType::OpCreate2,
                            TransferKind::SelfDestruct => OperationType::OpSelfDestruct,
                        },
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(internal_operations)
    }

    /// Handler for `ots_getTransactionError`
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<Option<Bytes>> {
        let maybe_revert = self
            .eth
            .spawn_trace_transaction_in_block_with_inspector(
                tx_hash,
                NoOpInspector,
                |_tx_info, _inspector, res, _| match res.result {
                    ExecutionResult::Revert { output, .. } => Ok(Some(output)),
                    _ => Ok(None),
                },
            )
            .await
            .map(Option::flatten)?;
        Ok(maybe_revert)
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
        // retrieve full block and its receipts
        let block = self.eth.block_by_number(block_number, true);
        let receipts = self.eth.block_receipts(BlockId::Number(block_number));
        let (block, receipts) = futures::try_join!(block, receipts)?;

        let mut block = block.ok_or_else(|| internal_rpc_err("block not found"))?;
        let mut receipts = receipts.ok_or_else(|| internal_rpc_err("receipts not found"))?;

        // check if the number of transactions matches the number of receipts
        let tx_len = block.transactions.len();
        if tx_len != receipts.len() {
            return Err(internal_rpc_err(
                "the number of transactions does not match the number of receipts",
            ));
        }

        // make sure the block is full
        let BlockTransactions::Full(transactions) = &mut block.inner.transactions else {
            return Err(internal_rpc_err("block is not full"));
        };

        // Crop page
        let page_end = tx_len.saturating_sub(page_number * page_size);
        let page_start = page_end.saturating_sub(page_size);

        // Crop transactions
        *transactions = transactions.drain(page_start..page_end).collect::<Vec<_>>();

        // The input field returns only the 4 bytes method selector instead of the entire
        // calldata byte blob.
        for tx in transactions {
            if tx.input.len() > 4 {
                tx.input = tx.input.slice(..4);
            }
        }

        // Crop receipts and transform them into OtsTransactionReceipt
        let timestamp = u64::try_from(block.header.timestamp).unwrap_or(u64::MAX);
        let receipts = receipts
            .drain(page_start..page_end)
            .map(|receipt| OtsTransactionReceipt { receipt, timestamp })
            .collect();
        Ok(OtsBlockTransactions { fullblock: block.inner.into(), receipts })
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
