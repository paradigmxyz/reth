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
    use reth_primitives::{hex_literal::hex, Bytes};
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::test_utils::testing_pool;

    use crate::EthApi;
    use std::sync::Arc;

    #[tokio::test]
    async fn send_raw_transaction() {
        let noop_provider = NoopProvider::default();

        let pool = testing_pool();

        let eth_api = EthApi::new(Arc::new(noop_provider), pool.clone(), ());

        let tx_1 = Bytes::from(hex!("f88b8212b085028fa6ae00830f424094aad593da0c8116ef7d2d594dd6a63241bccfc26c80a48318b64b000000000000000000000000641c5d790f862a58ec7abcfd644c0442e9c201b32aa0a6ef9e170bca5ffb7ac05433b13b7043de667fbb0b4a5e45d3b54fb2d6efcc63a0037ec2c05c3d60c5f5f78244ce0a3859e3a18a36c61efb061b383507d3ce19d2"));

        let tx_2 = Bytes::from(hex!("f86f8201e7852e90edd00083030d4094bd064928cdd4fd67fb99917c880e6560978d7ca1880de0b6b3a76400008025a07e833413ead52b8c538001b12ab5a85bac88db0b34b61251bb0fc81573ca093fa049634f1e439e3760265888434a2f9782928362412030db1429458ddc9dcee995"));

        eth_api.send_raw_transaction(tx_1).await.unwrap();
        eth_api.send_raw_transaction(tx_2).await.unwrap();

        assert!(!pool.is_empty());
        assert_eq!(pool.len(), 2);
    }
}
