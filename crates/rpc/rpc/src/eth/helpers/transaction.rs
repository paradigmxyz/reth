//! Contains RPC handler implementations specific to transactions

use std::sync::Arc;

use reth_provider::{BlockReaderIdExt, TransactionsProvider};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    RawTransactionForwarder,
};
use reth_rpc_eth_types::EthStateCache;
use reth_transaction_pool::TransactionPool;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthTransactions
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadTransaction,
    Pool: TransactionPool + 'static,
    Provider: BlockReaderIdExt,
{
    #[inline]
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    #[inline]
    fn raw_tx_forwarder(&self) -> Option<Arc<dyn RawTransactionForwarder>> {
        self.inner.raw_tx_forwarder()
    }

    #[inline]
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>> {
        self.inner.signers()
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadTransaction
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking,
    Provider: TransactionsProvider,
    Pool: TransactionPool,
{
    type Pool = Pool;

    #[inline]
    fn provider(&self) -> impl TransactionsProvider {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    #[inline]
    fn pool(&self) -> &Self::Pool {
        self.inner.pool()
    }
}

#[cfg(test)]
mod tests {
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, hex_literal::hex, Bytes};
    use reth_provider::test_utils::NoopProvider;
    use reth_rpc_eth_api::helpers::EthTransactions;
    use reth_rpc_eth_types::{
        EthStateCache, FeeHistoryCache, FeeHistoryCacheConfig, GasPriceOracle,
    };
    use reth_rpc_server_types::constants::{DEFAULT_ETH_PROOF_WINDOW, DEFAULT_PROOF_PERMITS};
    use reth_tasks::pool::BlockingTaskPool;
    use reth_transaction_pool::{test_utils::testing_pool, TransactionPool};

    use super::*;

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
            DEFAULT_ETH_PROOF_WINDOW,
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            fee_history_cache,
            evm_config,
            None,
            DEFAULT_PROOF_PERMITS,
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
