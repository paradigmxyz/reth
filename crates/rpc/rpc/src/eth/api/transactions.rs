//! Contains RPC handler implementations specific to transactions
use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{BlockId, Bytes, FromRecoveredTransaction, TransactionSigned, H256, U256};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rlp::Decodable;
use reth_rpc_types::{Index, Transaction, TransactionRequest};
use reth_transaction_pool::{TransactionOrigin, TransactionPool};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: 'static,
{
    pub(crate) async fn send_transaction(&self, _request: TransactionRequest) -> EthResult<H256> {
        unimplemented!()
    }

    /// Finds a given trasaction by its hash.
    pub(crate) async fn transaction_by_hash(
        &self,
        hash: H256,
    ) -> EthResult<Option<reth_rpc_types::Transaction>> {
        let tx_by_hash = match self.client().transaction_by_hash(hash)? {
            Some(item) => item,
            None => return Ok(None),
        };

        if let Some(tx) = tx_by_hash.into_ecrecovered() {
            Ok(Some(Transaction::from_recovered_with_block_context(
                tx,
                // TODO: this is just stubbed out for now still need to fully implement tx => block
                H256::default(),
                u64::default(),
                U256::from(usize::from(Index::default())),
            )))
        } else {
            Ok(None)
        }
    }

    /// Get Transaction by Block and tx index within that Block.
    /// Used for both:
    ///     transaction_by_block_hash_and_index() &
    ///     transaction_by_block_number_and_index()
    pub(crate) async fn transaction_by_block_and_tx_index(
        &self,
        block_id: impl Into<BlockId>,
        index: Index,
    ) -> EthResult<Option<reth_rpc_types::Transaction>> {
        let block_id = block_id.into();
        let Some(block) = self.client().block(block_id)? else {
            return Ok(None);
        };
        let block_hash =
            self.client().block_hash_for_id(block_id)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let Some(tx_signed) = block.body.into_iter().nth(usize::from(index)) else {
            return Ok(None);
        };
        let Some(tx) = tx_signed.into_ecrecovered() else {
            return Ok(None);
        };
        Ok(Some(Transaction::from_recovered_with_block_context(
            tx,
            block_hash,
            block.header.number,
            U256::from(usize::from(index)),
        )))
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
    use crate::eth::cache::EthStateCache;
    use reth_primitives::{hex_literal::hex, Bytes};
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::{test_utils::testing_pool, TransactionPool};

    use crate::EthApi;

    #[tokio::test]
    async fn send_raw_transaction() {
        let noop_provider = NoopProvider::default();

        let pool = testing_pool();

        let eth_api = EthApi::new(
            noop_provider,
            pool.clone(),
            (),
            EthStateCache::spawn(NoopProvider::default(), Default::default()),
        );

        // https://etherscan.io/tx/0xa694b71e6c128a2ed8e2e0f6770bddbe52e3bb8f10e8472f9a79ab81497a8b5d
        let tx_1 = Bytes::from(hex!("02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3"));

        let tx_1_result = eth_api.send_raw_transaction(tx_1).await.unwrap();
        assert_eq!(
            pool.len(),
            1,
            "expect 1 transactions in the pool, but pool size is {}",
            pool.len()
        );

        // https://etherscan.io/tx/0x48816c2f32c29d152b0d86ff706f39869e6c1f01dc2fe59a3c1f9ecf39384694
        let tx_2 = Bytes::from(hex!("02f9043c018202b7843b9aca00850c807d37a08304d21d94ef1c6e67703c7bd7107eed8303fbe6ec2554bf6b881bc16d674ec80000b903c43593564c000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000063e2d99f00000000000000000000000000000000000000000000000000000000000000030b000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000001e0000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000065717fe021ea67801d1088cc80099004b05b64600000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002bc02aaa39b223fe8d0a0e5c4f27ead9083c756cc20001f4a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000180000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000009e95fd5965fd1f1a6f0d4600000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000428dca9537116148616a5a3e44035af17238fe9dc080a0c6ec1e41f5c0b9511c49b171ad4e04c6bb419c74d99fe9891d74126ec6e4e879a032069a753d7a2cfa158df95421724d24c0e9501593c09905abf3699b4a4405ce"));

        let tx_2_result = eth_api.send_raw_transaction(tx_2).await.unwrap();
        assert_eq!(
            pool.len(),
            2,
            "expect 2 transactions in the pool, but pool size is {}",
            pool.len()
        );

        assert!(pool.get(&tx_1_result).is_some(), "tx1 not found in the pool");
        assert!(pool.get(&tx_2_result).is_some(), "tx2 not found in the pool");
    }
}
