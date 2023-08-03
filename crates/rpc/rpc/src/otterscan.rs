#![allow(dead_code, unused_variables)]
use crate::result::internal_rpc_err;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{Address, BlockId, BlockNumberOrTag, TxHash, H256};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_types::{
    BlockDetails, ContractCreator, InternalOperation, OtsBlockTransactions, TraceEntry,
    Transaction, TransactionsWithReceipts,
};

const API_LEVEL: u64 = 8;

/// Otterscan Api
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
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_getTransactionError`
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<String> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_traceTransaction`
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<TraceEntry> {
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
    async fn get_block_details_by_hash(&self, block_hash: H256) -> RpcResult<Option<BlockDetails>> {
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
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `searchTransactionsBefore`
    async fn search_transactions_before(
        &self,
        address: Address,
        block_number: BlockNumberOrTag,
        page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `searchTransactionsAfter`
    async fn search_transactions_after(
        &self,
        address: Address,
        block_number: BlockNumberOrTag,
        page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getTransactionBySenderAndNonce`
    async fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> RpcResult<Option<Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getContractCreator`
    async fn get_contract_creator(&self, address: Address) -> RpcResult<Option<ContractCreator>> {
        Err(internal_rpc_err("unimplemented"))
    }
}
