#![allow(dead_code, unused_variables)]
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{Address, BlockId, TxHash, H256, U256};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_types::{
    BlockDetails, ContractCreator, InternalOperation, TraceEntry, Transaction,
    TransactionsWithReceipts,
};

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
    async fn has_code(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<bool> {
        todo!()
    }

    async fn get_api_level(&self) -> RpcResult<u64> {
        todo!()
    }
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>> {
        todo!()
    }
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<String> {
        todo!()
    }
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<TraceEntry> {
        todo!()
    }
    async fn get_block_details(&self, block_number: U256) -> RpcResult<BlockDetails> {
        todo!()
    }
    async fn get_block_details_by_hash(&self, block_hash: H256) -> RpcResult<BlockDetails> {
        todo!()
    }
    async fn get_block_transactions(
        &self,
        block_number: U256,
        page_number: u8,
        page_size: u8,
    ) -> RpcResult<Vec<Transaction>> {
        todo!()
    }

    async fn search_transactions_before(
        &self,
        address: Address,
        block_number: U256,
        page_size: u8,
    ) -> RpcResult<TransactionsWithReceipts> {
        todo!()
    }
    async fn search_transactions_after(
        &self,
        address: Address,
        block_number: U256,
        page_size: u8,
    ) -> RpcResult<TransactionsWithReceipts> {
        todo!()
    }
    async fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> RpcResult<Transaction> {
        todo!()
    }
    async fn get_contract_creator(&self, address: Address) -> RpcResult<ContractCreator> {
        todo!()
    }
}
