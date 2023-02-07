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

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
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
