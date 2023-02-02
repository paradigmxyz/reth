//! Contains RPC handler implementations specific to transactions

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{Bytes, FromRecoveredTransaction, TransactionSigned, H256};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rlp::Decodable;
use reth_rpc_types::TransactionRequest;
use reth_transaction_pool::{TransactionOrigin, TransactionPool};

impl<Pool, Client, Network> EthApi<Pool, Client, Network>
where
    Pool: TransactionPool + 'static,
    Client: BlockProvider + StateProviderFactory + 'static,
    Network: 'static,
{
    pub(crate) async fn send_transaction(&self, _request: TransactionRequest) -> EthResult<H256> {
        unimplemented!()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    pub(crate) async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<H256> {
        let mut data = tx.as_ref();
        if data.is_empty() {
            return Err(EthApiError::EmptyRawTransactionData)
        }

        let transaction = TransactionSigned::decode(&mut data)
            .map_err(|_| EthApiError::FailedToDecodeSignedTransaction)?;

        let recovered =
            transaction.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

        let pool_transaction = <Pool::Transaction>::from_recovered_transaction(recovered);

        // submit the transaction to the pool with a `Local` origin
        let hash = self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

        Ok(hash)
    }
}


#[cfg(test)]
mod tests {
    use reth_transaction_pool::test_utils::testing_pool;
    use reth_primitives::Bytes;

    use crate::EthApi;
    use test_utils::NoopClient;
    use std::{sync::Arc, str::FromStr};



    #[tokio::test]
    async fn send_raw_transaction() {

        let pool = testing_pool();

        let client = NoopClient::default();

        let eth_api = EthApi::new(Arc::new(client), pool.clone(), ());

        let tx = Bytes::from_str("0x123456789abcdef").unwrap();

        eth_api.send_raw_transaction(tx).await.unwrap();

        assert!(!pool.is_empty());

    }



    mod test_utils {
        use reth_provider::{BlockProvider, StateProviderFactory, BlockHashProvider, StateProvider, AccountProvider};
        use reth_interfaces::Result;
        use reth_primitives::{ChainInfo, U256, rpc::BlockId, Block};

        #[derive(Debug, Default)]
        pub(crate) struct NoopClient;

        pub(crate) struct NoopSP;

        impl AccountProvider for NoopClient {
            fn basic_account(&self,address:reth_primitives::Address) -> Result<Option<reth_primitives::Account> > {
                todo!();
            }
        }

        impl StateProvider for NoopClient {
            fn storage(&self,account:reth_primitives::Address,storage_key:reth_primitives::StorageKey) -> Result<Option<reth_primitives::StorageValue> > {
                todo!();
            }

            fn bytecode_by_hash(&self,code_hash:reth_primitives::H256) -> Result<Option<reth_primitives::Bytes> > {
                todo!();
            }

        }


        impl BlockHashProvider for NoopClient {

            fn block_hash(&self,number:U256) -> Result<Option<reth_primitives::H256> > {
                todo!();
            }

        }

        impl BlockProvider for NoopClient {

            fn chain_info(&self) -> Result<ChainInfo> {
                todo!();
            }

            fn block_number(&self, hash: reth_primitives::H256) -> Result<Option<reth_primitives::BlockNumber>> {
                todo!();
            }

            fn block(&self, id: BlockId) -> Result<Option<Block>> {
                todo!();
            }

        }

        impl StateProviderFactory for NoopClient {

            type HistorySP<'a> = NoopClient where Self: 'a;
            type LatestSP<'a> = NoopClient where Self: 'a;

            fn latest(&self) -> Result<Self::LatestSP<'_>> {
                todo!();
            }
            
            fn history_by_block_number(&self, block: reth_primitives::BlockNumber) -> Result<Self::HistorySP<'_>> {
                todo!();
            }

            fn history_by_block_hash(&self, block: reth_primitives::BlockHash) -> Result<Self::HistorySP<'_>> {
                todo!();
            }
        }


    }

}

