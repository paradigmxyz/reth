//! Contains RPC handler implementations specific to transactions

use reth_primitives::{TransactionSignedEcRecovered, B256};
use reth_rpc_types::{Transaction, TransactionInfo};
use reth_rpc_types_compat::transaction::from_recovered_with_block_context;

use crate::EthApi;

/// Implements [`EthTransactions`](crate::eth::api::EthTransactions) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_transactions_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::EthTransactions for $network_api
        where
            Self: $crate::eth::api::LoadTransaction,
            Pool: reth_transaction_pool::TransactionPool + 'static,
            Provider: reth_provider::BlockReaderIdExt,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::BlockReaderIdExt {
                self.inner.provider()
            }

            #[inline]
            fn raw_tx_forwarder(
                &self,
            ) -> Option<std::sync::Arc<dyn $crate::eth::api::RawTransactionForwarder>> {
                self.inner.raw_tx_forwarder()
            }

            #[inline]
            fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn $crate::eth::EthSigner>>> {
                self.inner.signers()
            }
        }
    };
}

/// Implements [`LoadTransaction`](crate::eth::api::LoadTransaction) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_transaction_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::eth::api::LoadTransaction for $network_api
        where
            Self: $crate::eth::api::SpawnBlocking,
            Provider: reth_provider::TransactionsProvider,
            Pool: reth_transaction_pool::TransactionPool,
        {
            type Pool = Pool;

            #[inline]
            fn provider(&self) -> impl reth_provider::TransactionsProvider {
                self.inner.provider()
            }

            #[inline]
            fn cache(&self) -> &$crate::eth::cache::EthStateCache {
                self.inner.cache()
            }

            #[inline]
            fn pool(&self) -> &Self::Pool {
                self.inner.pool()
            }
        }
    };
}

eth_transactions_impl!(EthApi<Provider, Pool, Network, EvmConfig>);
load_transaction_impl!(EthApi<Provider, Pool, Network, EvmConfig>);

/// Represents from where a transaction was fetched.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransactionSource {
    /// Transaction exists in the pool (Pending)
    Pool(TransactionSignedEcRecovered),
    /// Transaction already included in a block
    ///
    /// This can be a historical block or a pending block (received from the CL)
    Block {
        /// Transaction fetched via provider
        transaction: TransactionSignedEcRecovered,
        /// Index of the transaction in the block
        index: u64,
        /// Hash of the block.
        block_hash: B256,
        /// Number of the block.
        block_number: u64,
        /// base fee of the block.
        base_fee: Option<u64>,
    },
}

// === impl TransactionSource ===

impl TransactionSource {
    /// Consumes the type and returns the wrapped transaction.
    pub fn into_recovered(self) -> TransactionSignedEcRecovered {
        self.into()
    }

    /// Returns the transaction and block related info, if not pending
    pub fn split(self) -> (TransactionSignedEcRecovered, TransactionInfo) {
        match self {
            Self::Pool(tx) => {
                let hash = tx.hash();
                (
                    tx,
                    TransactionInfo {
                        hash: Some(hash),
                        index: None,
                        block_hash: None,
                        block_number: None,
                        base_fee: None,
                    },
                )
            }
            Self::Block { transaction, index, block_hash, block_number, base_fee } => {
                let hash = transaction.hash();
                (
                    transaction,
                    TransactionInfo {
                        hash: Some(hash),
                        index: Some(index),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        base_fee: base_fee.map(u128::from),
                    },
                )
            }
        }
    }
}

impl From<TransactionSource> for TransactionSignedEcRecovered {
    fn from(value: TransactionSource) -> Self {
        match value {
            TransactionSource::Pool(tx) => tx,
            TransactionSource::Block { transaction, .. } => transaction,
        }
    }
}

impl From<TransactionSource> for Transaction {
    fn from(value: TransactionSource) -> Self {
        match value {
            TransactionSource::Pool(tx) => reth_rpc_types_compat::transaction::from_recovered(tx),
            TransactionSource::Block { transaction, index, block_hash, block_number, base_fee } => {
                from_recovered_with_block_context(
                    transaction,
                    block_hash,
                    block_number,
                    base_fee,
                    index as usize,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::{
        api::EthTransactions, cache::EthStateCache, gas_oracle::GasPriceOracle, FeeHistoryCache,
        FeeHistoryCacheConfig,
    };
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, hex_literal::hex, Bytes};
    use reth_provider::test_utils::NoopProvider;
    use reth_tasks::pool::BlockingTaskPool;
    use reth_transaction_pool::{test_utils::testing_pool, TransactionPool};

    #[tokio::test]
    async fn send_raw_transaction() {
        let noop_provider = NoopProvider::default();
        let noop_network_provider = NoopNetwork::default();

        let pool = testing_pool();

        let evm_config = EthEvmConfig::default();
        let cache = EthStateCache::spawn(noop_provider, Default::default(), evm_config);
        let fee_history_cache =
            FeeHistoryCache::new(cache.clone(), FeeHistoryCacheConfig::default());
        let eth_api = EthApi::new(
            noop_provider,
            pool.clone(),
            noop_network_provider,
            cache.clone(),
            GasPriceOracle::new(noop_provider, Default::default(), cache.clone()),
            ETHEREUM_BLOCK_GAS_LIMIT,
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            fee_history_cache,
            evm_config,
            None,
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
