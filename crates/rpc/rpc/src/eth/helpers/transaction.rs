//! Contains RPC handler implementations specific to transactions

use std::time::Duration;

use crate::EthApi;
use alloy_consensus::BlobTransactionValidationError;
use alloy_eips::{eip7594::BlobTransactionSidecarVariant, BlockId, Typed2718};
use alloy_primitives::{hex, Bytes, B256};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_primitives_traits::AlloyBlockHeader;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{spec::SignersForRpc, EthTransactions, LoadTransaction},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::{error::RpcPoolError, utils::recover_raw_transaction, EthApiError};
use reth_storage_api::BlockReaderIdExt;
use reth_transaction_pool::{
    error::Eip4844PoolTransactionError, AddedTransactionOutcome, EthBlobTransactionSidecar,
    EthPoolTransaction, PoolTransaction, TransactionPool,
};

impl<N, Rpc> EthTransactions for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
    #[inline]
    fn signers(&self) -> &SignersForRpc<Self::Provider, Self::NetworkTypes> {
        self.inner.signers()
    }

    #[inline]
    fn send_raw_transaction_sync_timeout(&self) -> Duration {
        self.inner.send_raw_transaction_sync_timeout()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;

        let mut pool_transaction =
            <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);

        // TODO: remove this after Osaka transition
        // Convert legacy blob sidecars to EIP-7594 format
        if pool_transaction.is_eip4844() {
            let EthBlobTransactionSidecar::Present(sidecar) = pool_transaction.take_blob() else {
                return Err(EthApiError::PoolError(RpcPoolError::Eip4844(
                    Eip4844PoolTransactionError::MissingEip4844BlobSidecar,
                )));
            };

            let sidecar = match sidecar {
                BlobTransactionSidecarVariant::Eip4844(sidecar) => {
                    let latest = self
                        .provider()
                        .latest_header()?
                        .ok_or(EthApiError::HeaderNotFound(BlockId::latest()))?;
                    // Convert to EIP-7594 if next block is Osaka
                    if self
                        .provider()
                        .chain_spec()
                        .is_osaka_active_at_timestamp(latest.timestamp().saturating_add(12))
                    {
                        BlobTransactionSidecarVariant::Eip7594(
                            self.blob_sidecar_converter().convert(sidecar).await.ok_or_else(
                                || {
                                    RpcPoolError::Eip4844(
                                        Eip4844PoolTransactionError::InvalidEip4844Blob(
                                            BlobTransactionValidationError::InvalidProof,
                                        ),
                                    )
                                },
                            )?,
                        )
                    } else {
                        BlobTransactionSidecarVariant::Eip4844(sidecar)
                    }
                }
                sidecar => sidecar,
            };

            pool_transaction =
                EthPoolTransaction::try_from_eip4844(pool_transaction.into_consensus(), sidecar)
                    .ok_or_else(|| {
                        RpcPoolError::Eip4844(
                            Eip4844PoolTransactionError::MissingEip4844BlobSidecar,
                        )
                    })?;
        }

        // forward the transaction to the specific endpoint if configured.
        if let Some(client) = self.raw_tx_forwarder() {
            tracing::debug!(target: "rpc::eth", hash = %pool_transaction.hash(), "forwarding raw transaction to forwarder");
            let rlp_hex = hex::encode_prefixed(&tx);

            // broadcast raw transaction to subscribers if there is any.
            self.broadcast_raw_transaction(tx);

            let hash =
                client.request("eth_sendRawTransaction", (rlp_hex,)).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                }).map_err(EthApiError::other)?;

            // Retain tx in local tx pool after forwarding, for local RPC usage.
            let _ = self.inner.add_pool_transaction(pool_transaction).await;

            return Ok(hash);
        }

        // broadcast raw transaction to subscribers if there is any.
        self.broadcast_raw_transaction(tx);

        // submit the transaction to the pool with a `Local` origin
        let AddedTransactionOutcome { hash, .. } =
            self.inner.add_pool_transaction(pool_transaction).await?;

        Ok(hash)
    }
}

impl<N, Rpc> LoadTransaction for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::helpers::types::EthRpcConverter;
    use alloy_consensus::{Block, Header, SidecarBuilder, SimpleCoder, Transaction};
    use alloy_primitives::{Address, U256};
    use alloy_rpc_types_eth::request::TransactionRequest;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use std::collections::HashMap;

    fn mock_eth_api(
        accounts: HashMap<Address, ExtendedAccount>,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let mock_provider = MockEthProvider::default()
            .with_chain_spec(ChainSpecBuilder::mainnet().cancun_activated().build());
        mock_provider.extend_accounts(accounts);

        let evm_config = EthEvmConfig::new(mock_provider.chain_spec());
        let pool = testing_pool();

        let genesis_header = Header {
            number: 0,
            gas_limit: 30_000_000,
            timestamp: 1,
            excess_blob_gas: Some(0),
            base_fee_per_gas: Some(1000000000),
            blob_gas_used: Some(0),
            ..Default::default()
        };

        let genesis_hash = B256::ZERO;
        mock_provider.add_block(genesis_hash, Block::new(genesis_header, Default::default()));

        EthApi::builder(mock_provider, pool, NoopNetwork::default(), evm_config).build()
    }

    #[tokio::test]
    async fn send_raw_transaction() {
        let eth_api = mock_eth_api(Default::default());
        let pool = eth_api.pool();

        // https://etherscan.io/tx/0xa694b71e6c128a2ed8e2e0f6770bddbe52e3bb8f10e8472f9a79ab81497a8b5d
        let tx_1 = Bytes::from(hex!(
            "02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3"
        ));

        let tx_1_result = eth_api.send_raw_transaction(tx_1).await.unwrap();
        assert_eq!(
            pool.len(),
            1,
            "expect 1 transaction in the pool, but pool size is {}",
            pool.len()
        );

        // https://etherscan.io/tx/0x48816c2f32c29d152b0d86ff706f39869e6c1f01dc2fe59a3c1f9ecf39384694
        let tx_2 = Bytes::from(hex!(
            "02f9043c018202b7843b9aca00850c807d37a08304d21d94ef1c6e67703c7bd7107eed8303fbe6ec2554bf6b881bc16d674ec80000b903c43593564c000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000063e2d99f00000000000000000000000000000000000000000000000000000000000000030b000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000001e0000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000065717fe021ea67801d1088cc80099004b05b64600000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002bc02aaa39b223fe8d0a0e5c4f27ead9083c756cc20001f4a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000180000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000009e95fd5965fd1f1a6f0d4600000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000428dca9537116148616a5a3e44035af17238fe9dc080a0c6ec1e41f5c0b9511c49b171ad4e04c6bb419c74d99fe9891d74126ec6e4e879a032069a753d7a2cfa158df95421724d24c0e9501593c09905abf3699b4a4405ce"
        ));

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

    #[tokio::test]
    async fn test_fill_transaction_fills_chain_id() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)), // 10 ETH
        )]);

        let eth_api = mock_eth_api(accounts);

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            gas: Some(21_000),
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        // Should fill with the chain id from provider
        assert!(filled.tx.chain_id().is_some());
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_nonce() {
        let address = Address::random();
        let nonce = 42u64;

        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(nonce, U256::from(1_000_000_000_000_000_000u64)), // 1 ETH
        )]);

        let eth_api = mock_eth_api(accounts);

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            value: Some(U256::from(1000)),
            gas: Some(21_000),
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        assert_eq!(filled.tx.nonce(), nonce);
    }

    #[tokio::test]
    async fn test_fill_transaction_preserves_provided_fields() {
        let address = Address::random();
        let provided_nonce = 100u64;
        let provided_gas_limit = 50_000u64;

        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(42, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            value: Some(U256::from(1000)),
            nonce: Some(provided_nonce),
            gas: Some(provided_gas_limit),
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        // Should preserve the provided nonce and gas limit
        assert_eq!(filled.tx.nonce(), provided_nonce);
        assert_eq!(filled.tx.gas_limit(), provided_gas_limit);
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_all_missing_fields() {
        let address = Address::random();

        let balance = U256::from(100u128) * U256::from(1_000_000_000_000_000_000u128);
        let accounts = HashMap::from([(address, ExtendedAccount::new(5, balance))]);

        let eth_api = mock_eth_api(accounts);

        // Create a simple transfer transaction
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        assert!(filled.tx.is_eip1559());
    }

    #[tokio::test]
    async fn test_fill_transaction_eip4844_blob_fee() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");

        // EIP-4844 blob transaction with versioned hashes but no blob fee
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            sidecar: Some(builder.build().unwrap()),
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        // Blob transaction should have max_fee_per_blob_gas filled
        assert!(
            filled.tx.max_fee_per_blob_gas().is_some(),
            "max_fee_per_blob_gas should be filled for blob tx"
        );
        assert!(
            filled.tx.blob_versioned_hashes().is_some(),
            "blob_versioned_hashes should be preserved"
        );
    }

    #[tokio::test]
    async fn test_fill_transaction_eip4844_preserves_blob_fee() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        let provided_blob_fee = 5000000u128;

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");

        // EIP-4844 blob transaction with blob fee already set
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(3), // EIP-4844
            sidecar: Some(builder.build().unwrap()),
            max_fee_per_blob_gas: Some(provided_blob_fee), // Already set
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        // Should preserve the provided blob fee
        assert_eq!(
            filled.tx.max_fee_per_blob_gas(),
            Some(provided_blob_fee),
            "should preserve provided max_fee_per_blob_gas"
        );
    }

    #[tokio::test]
    async fn test_fill_transaction_non_blob_tx_no_blob_fee() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // EIP-1559 transaction without blob fields
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(2), // EIP-1559
            ..Default::default()
        };

        let filled =
            eth_api.fill_transaction(tx_req).await.expect("fill_transaction should succeed");

        // Non-blob transaction should NOT have blob fee filled
        assert!(
            filled.tx.max_fee_per_blob_gas().is_none(),
            "max_fee_per_blob_gas should not be set for non-blob tx"
        );
    }
}
