//! Contains RPC handler implementations specific to transactions

use crate::EthApi;
use alloy_consensus::BlobTransactionValidationError;
use alloy_eips::{eip7594::BlobTransactionSidecarVariant, BlockId, Typed2718};
use alloy_primitives::{hex, B256};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_primitives_traits::{AlloyBlockHeader, WithEncoded};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{spec::SignersForRpc, EthTransactions, LoadTransaction},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::{error::RpcPoolError, EthApiError};
use reth_storage_api::BlockReaderIdExt;
use reth_transaction_pool::{
    error::Eip4844PoolTransactionError, AddedTransactionOutcome, EthBlobTransactionSidecar,
    EthPoolTransaction, PoolTransaction, PoolTx,
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

    async fn send_pool_transaction(
        &self,
        origin: reth_transaction_pool::TransactionOrigin,
        tx: WithEncoded<PoolTx<Self::Pool>>,
    ) -> Result<B256, Self::Error> {
        let (tx, mut pool_transaction) = tx.split();

        // Optionally convert legacy blob sidecars to EIP-7594 format when Osaka is active
        // This is opt-in via --rpc.force-blob-sidecar-upcasting
        if self.inner.force_blob_sidecar_upcasting() && pool_transaction.is_eip4844() {
            let EthBlobTransactionSidecar::Present(sidecar) = pool_transaction.take_blob() else {
                return Err(EthApiError::PoolError(RpcPoolError::Eip4844(
                    Eip4844PoolTransactionError::MissingEip4844BlobSidecar,
                )));
            };
            let sidecar = sidecar.into_sidecar();

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
            let _ = self.inner.add_pool_transaction(origin, pool_transaction).await;

            return Ok(hash);
        }

        // broadcast raw transaction to subscribers if there is any.
        self.broadcast_raw_transaction(tx);

        let AddedTransactionOutcome { hash, .. } =
            self.inner.add_pool_transaction(origin, pool_transaction).await?;

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
    use std::time::Duration;

    use super::*;
    use crate::eth::helpers::{signer::DevSigner, types::EthRpcConverter};
    use alloy_consensus::{BlobTransactionSidecar, Block, Header, Transaction, TxEip8141};
    use alloy_eips::{
        eip2718::Encodable2718,
        eip4844::{Blob, Bytes48},
        eip8141::{Frame, FrameAddress, FrameLimits, FrameMode, FrameStatus, TransactionFees},
        eip8272::{RECENT_ROOT_ADDRESS, RECENT_ROOT_CODE, RECENT_ROOT_LENGTH},
    };
    use alloy_primitives::{bytes, map::AddressMap, Address, Bytes, Signature, U256};
    use alloy_rpc_types_eth::{
        request::TransactionRequest, state::EvmOverrides, BlockOverrides, FrameRequest,
    };
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::SenderRecoveryCache;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives_traits::SignedTransaction;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{
        helpers::{EthCall, SpawnBlocking},
        node::RpcNodeCoreAdapter,
    };
    use reth_rpc_eth_types::{FrameSimulationPrefixShape, GasCap};
    use reth_transaction_pool::{
        test_utils::{testing_pool, TestPool, TransactionGenerator},
        validate::RecentRootReference,
        PoolPooledTx, TransactionOrigin, TransactionPool,
    };

    /// A sidecar with one zero blob. Its commitment and proof are the point at infinity, so it is
    /// valid without loading the KZG trusted setup.
    fn zero_blob_sidecar() -> BlobTransactionSidecar {
        let mut point_at_infinity = [0; 48];
        point_at_infinity[0] = 0xc0;
        BlobTransactionSidecar::new(
            vec![Blob::default()],
            vec![Bytes48::from(point_at_infinity)],
            vec![Bytes48::from(point_at_infinity)],
        )
    }
    fn mock_eth_api(
        accounts: AddressMap<ExtendedAccount>,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        mock_eth_api_with_sync_timeout(accounts, Duration::from_secs(30))
    }

    fn mock_eth_api_with_sync_timeout(
        accounts: AddressMap<ExtendedAccount>,
        send_raw_transaction_sync_timeout: Duration,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        mock_eth_api_builder(accounts, ChainSpecBuilder::mainnet().cancun_activated().build())
            .send_raw_transaction_sync_timeout(send_raw_transaction_sync_timeout)
            .build()
    }

    fn mock_eth_api_builder(
        accounts: AddressMap<ExtendedAccount>,
        chain_spec: ChainSpec,
    ) -> crate::EthApiBuilder<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let mock_provider = MockEthProvider::default().with_chain_spec(chain_spec);
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
            parent_beacon_block_root: Some(B256::ZERO),
            slot_number: mock_provider.chain_spec().is_bogota_active_at_timestamp(1).then_some(1),
            ..Default::default()
        };

        let genesis_hash = genesis_header.hash_slow();
        mock_provider.add_block(genesis_hash, Block::new(genesis_header, Default::default()));

        EthApi::builder(mock_provider, pool, NoopNetwork::default(), evm_config)
    }

    fn mock_frame_eth_api(
        accounts: AddressMap<ExtendedAccount>,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        mock_eth_api_builder(accounts, ChainSpecBuilder::mainnet().bogota_activated().build())
            .send_raw_transaction_sync_timeout(Duration::from_secs(30))
            .build()
    }

    fn raw_transfer_tx() -> Bytes {
        // https://etherscan.io/tx/0xa694b71e6c128a2ed8e2e0f6770bddbe52e3bb8f10e8472f9a79ab81497a8b5d
        bytes!("02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3")
    }

    #[tokio::test]
    async fn raw_transaction_recovery_shares_sender_cache() {
        let cache = SenderRecoveryCache::new(16);
        let eth_api = mock_eth_api_builder(
            Default::default(),
            ChainSpecBuilder::mainnet().cancun_activated().build(),
        )
        .sender_recovery_cache(Some(cache.clone()))
        .build();
        let mut generator = TransactionGenerator::new(rand::rng());

        for (transaction, warm_cache) in [
            (generator.transaction().into_legacy(), false),
            (generator.transaction().nonce(1).into_eip1559(), true),
        ] {
            let raw = Bytes::from(transaction.encoded_2718());
            let hash = *transaction.tx_hash();
            let sender = transaction.try_recover().unwrap();
            assert_eq!(cache.get(&hash), None);
            if warm_cache {
                let recovered =
                    eth_api.recover_raw_transaction::<PoolPooledTx<TestPool>>(&raw).unwrap();
                assert_eq!(recovered.signer(), sender);
                assert_eq!(cache.get(&hash), Some(sender));
            }

            assert_eq!(eth_api.send_raw_transaction(raw.clone()).await.unwrap(), hash);
            assert_eq!(cache.get(&hash), Some(sender));
            let recovered =
                eth_api.recover_raw_transaction::<PoolPooledTx<TestPool>>(&raw).unwrap();
            assert_eq!(recovered.signer(), sender);
            assert_eq!(eth_api.recover_raw_pool_transaction(&raw).unwrap().sender(), sender);
        }
    }

    #[tokio::test]
    async fn raw_transaction_recovery_uses_pool_overrides() {
        let raw = raw_transfer_tx();
        for enabled in [false, true] {
            let cache = SenderRecoveryCache::new(16);
            let eth_api = mock_eth_api_builder(
                Default::default(),
                ChainSpecBuilder::mainnet().cancun_activated().build(),
            )
            .sender_recovery_cache(enabled.then(|| cache.clone()))
            .build();

            // MockTransaction's raw hooks retain the input length instead of its in-memory size.
            for _ in 0..2 {
                let transaction = eth_api.recover_raw_pool_transaction(&raw).unwrap();
                assert_eq!(transaction.encoded_length(), raw.len());
                assert_eq!(cache.get(transaction.hash()), enabled.then_some(transaction.sender()));
            }
        }
    }

    #[tokio::test]
    async fn raw_transaction_recovery_preserves_errors() {
        let invalid = TransactionSigned::new_unhashed(
            alloy_consensus::TxLegacy::default().into(),
            Signature::new(U256::ZERO, U256::ZERO, false),
        );
        let mut trailing = raw_transfer_tx().to_vec();
        trailing.push(0);

        for enabled in [false, true] {
            let cache = SenderRecoveryCache::new(16);
            let eth_api = mock_eth_api_builder(
                Default::default(),
                ChainSpecBuilder::mainnet().cancun_activated().build(),
            )
            .sender_recovery_cache(enabled.then(|| cache.clone()))
            .build();
            let valid = eth_api.recover_raw_pool_transaction(&raw_transfer_tx()).unwrap();
            assert_eq!(cache.get(valid.hash()), enabled.then_some(valid.sender()));
            for (raw, expected) in [
                (vec![], EthApiError::EmptyRawTransactionData),
                (vec![2], EthApiError::FailedToDecodeSignedTransaction),
                (trailing.clone(), EthApiError::FailedToDecodeSignedTransaction),
                (invalid.encoded_2718(), EthApiError::InvalidTransactionSignature),
            ] {
                let err = eth_api.send_raw_transaction(raw.clone().into()).await.unwrap_err();
                assert_eq!(err.to_string(), expected.to_string());
                let err =
                    eth_api.recover_raw_transaction::<PoolPooledTx<TestPool>>(&raw).unwrap_err();
                assert_eq!(err.to_string(), expected.to_string());
            }
            assert!(eth_api.pool().is_empty());
            assert_eq!(cache.get(invalid.tx_hash()), None);
        }
    }

    #[tokio::test]
    async fn cached_raw_transaction_preserves_blob_sidecar() {
        let cache = SenderRecoveryCache::new(16);
        let provider = MockEthProvider::default();
        let evm_config = EthEvmConfig::new(provider.chain_spec());
        let eth_api = EthApi::builder(
            provider,
            reth_transaction_pool::noop::NoopTransactionPool::default(),
            NoopNetwork::default(),
            evm_config,
        )
        .sender_recovery_cache(Some(cache.clone()))
        .build();
        let sidecar = BlobTransactionSidecarVariant::from(zero_blob_sidecar());
        let transaction = TransactionSigned::new_unhashed(
            alloy_consensus::TxEip4844 {
                blob_versioned_hashes: sidecar.versioned_hashes().collect(),
                ..Default::default()
            }
            .into(),
            Signature::test_signature(),
        );
        let hash = *transaction.tx_hash();
        let sender = transaction.try_recover().unwrap();
        let raw = transaction.try_into_pooled_eip4844(sidecar.clone()).unwrap().encoded_2718();

        for _ in 0..2 {
            let mut recovered = eth_api.recover_raw_pool_transaction(&raw).unwrap();
            assert_eq!(recovered.sender(), sender);
            assert_eq!(cache.get(&hash), Some(sender));
            let EthBlobTransactionSidecar::Present(actual) = recovered.take_blob() else {
                panic!("missing blob sidecar")
            };
            assert_eq!(actual.into_sidecar(), sidecar);
        }
    }

    #[tokio::test]
    async fn send_raw_transaction() {
        let eth_api = mock_eth_api(Default::default());
        let pool = eth_api.pool();

        let tx_1 = raw_transfer_tx();

        let tx_1_result = eth_api.send_raw_transaction(tx_1).await.unwrap();
        assert_eq!(
            pool.len(),
            1,
            "expect 1 transaction in the pool, but pool size is {}",
            pool.len()
        );

        // https://etherscan.io/tx/0x48816c2f32c29d152b0d86ff706f39869e6c1f01dc2fe59a3c1f9ecf39384694
        let tx_2 = bytes!("02f9043c018202b7843b9aca00850c807d37a08304d21d94ef1c6e67703c7bd7107eed8303fbe6ec2554bf6b881bc16d674ec80000b903c43593564c000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000063e2d99f00000000000000000000000000000000000000000000000000000000000000030b000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000001e0000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000065717fe021ea67801d1088cc80099004b05b64600000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002bc02aaa39b223fe8d0a0e5c4f27ead9083c756cc20001f4a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000180000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000009e95fd5965fd1f1a6f0d4600000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000428dca9537116148616a5a3e44035af17238fe9dc080a0c6ec1e41f5c0b9511c49b171ad4e04c6bb419c74d99fe9891d74126ec6e4e879a032069a753d7a2cfa158df95421724d24c0e9501593c09905abf3699b4a4405ce");

        let tx_2_result = eth_api.send_raw_transaction(tx_2).await.unwrap();
        assert_eq!(
            pool.len(),
            2,
            "expect 2 transactions in the pool, but pool size is {}",
            pool.len()
        );

        assert!(pool.get(&tx_1_result).is_some(), "tx1 not found in the pool");
        assert!(pool.get(&tx_2_result).is_some(), "tx2 not found in the pool");
        assert_eq!(pool.get(&tx_1_result).unwrap().origin, TransactionOrigin::Local);
        assert_eq!(pool.get(&tx_2_result).unwrap().origin, TransactionOrigin::Local);
    }

    #[tokio::test]
    async fn send_raw_transaction_sync_uses_request_timeout() {
        let eth_api = mock_eth_api(Default::default());

        let err = eth_api.send_raw_transaction_sync(raw_transfer_tx(), Some(1)).await.unwrap_err();

        assert!(matches!(
            err,
            EthApiError::TransactionConfirmationTimeout { duration, .. }
                if duration == Duration::from_millis(1)
        ));
        assert_eq!(eth_api.pool().len(), 1);
    }

    #[tokio::test]
    async fn send_raw_transaction_sync_uses_configured_timeout_when_omitted() {
        let eth_api = mock_eth_api_with_sync_timeout(Default::default(), Duration::from_millis(1));

        let err = eth_api.send_raw_transaction_sync(raw_transfer_tx(), None).await.unwrap_err();

        assert!(matches!(
            err,
            EthApiError::TransactionConfirmationTimeout { duration, .. }
                if duration == Duration::from_millis(1)
        ));
        assert_eq!(eth_api.pool().len(), 1);
    }

    #[tokio::test]
    async fn send_raw_transaction_sync_uses_configured_timeout_when_zero() {
        let eth_api = mock_eth_api_with_sync_timeout(Default::default(), Duration::from_millis(1));

        let err = eth_api.send_raw_transaction_sync(raw_transfer_tx(), Some(0)).await.unwrap_err();

        assert!(matches!(
            err,
            EthApiError::TransactionConfirmationTimeout { duration, .. }
                if duration == Duration::from_millis(1)
        ));
        assert_eq!(eth_api.pool().len(), 1);
    }

    #[tokio::test]
    async fn send_raw_transaction_sync_caps_request_timeout() {
        let eth_api = mock_eth_api_with_sync_timeout(Default::default(), Duration::from_millis(1));

        let err = eth_api.send_raw_transaction_sync(raw_transfer_tx(), Some(50)).await.unwrap_err();

        assert!(matches!(
            err,
            EthApiError::TransactionConfirmationTimeout { duration, .. }
                if duration == Duration::from_millis(1)
        ));
        assert_eq!(eth_api.pool().len(), 1);
    }

    #[tokio::test]
    async fn send_transaction_preserves_provided_gas_limit() {
        let signers = DevSigner::random_signers(1);
        let address = signers[0].accounts()[0];
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let eth_api = mock_eth_api(accounts);
        eth_api.signers().write().extend(signers);

        let provided_gas_limit = 90_000;
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(address.into()),
            gas: Some(provided_gas_limit),
            gas_price: Some(1_000_000_000),
            ..Default::default()
        };

        let hash = eth_api
            .send_transaction_request(tx_req)
            .await
            .expect("send_transaction should succeed");
        let pooled = eth_api.pool().get(&hash).expect("transaction should be in the pool");

        assert_eq!(pooled.transaction.gas_limit(), provided_gas_limit);
    }

    #[tokio::test]
    async fn send_transaction_fills_missing_fee_fields() {
        let signers = DevSigner::random_signers(1);
        let address = signers[0].accounts()[0];
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let eth_api = mock_eth_api(accounts);
        eth_api.signers().write().extend(signers);

        // no gas, gasPrice or 1559 fee fields: the node is expected to fill them
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(address.into()),
            value: Some(U256::ONE),
            ..Default::default()
        };

        let hash = eth_api
            .send_transaction_request(tx_req)
            .await
            .expect("send_transaction should fill the missing fee fields");
        let pooled = eth_api.pool().get(&hash).expect("transaction should be in the pool");
        assert!(pooled.transaction.max_fee_per_gas() > 0);
    }

    #[tokio::test]
    async fn send_transaction_rejects_mismatched_chain_id() {
        let signers = DevSigner::random_signers(1);
        let address = signers[0].accounts()[0];
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let eth_api = mock_eth_api(accounts);
        eth_api.signers().write().extend(signers);

        // The mock node is mainnet (chain id 1); the caller pins a different chain.
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(address.into()),
            gas: Some(90_000),
            gas_price: Some(1_000_000_000),
            chain_id: Some(999),
            ..Default::default()
        };

        let err = eth_api
            .send_transaction_request(tx_req)
            .await
            .expect_err("a chain id that is not the node's must be rejected, not rewritten");
        assert!(
            err.to_string().contains("chainId does not match node's"),
            "unexpected error: {err}"
        );
        assert!(eth_api.pool().is_empty(), "no transaction should have been submitted");
    }

    #[tokio::test]
    async fn send_transaction_accepts_matching_chain_id() {
        let signers = DevSigner::random_signers(1);
        let address = signers[0].accounts()[0];
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let eth_api = mock_eth_api(accounts);
        eth_api.signers().write().extend(signers);

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(address.into()),
            gas: Some(90_000),
            gas_price: Some(1_000_000_000),
            chain_id: Some(1),
            ..Default::default()
        };

        let hash = eth_api
            .send_transaction_request(tx_req)
            .await
            .expect("a matching chain id must still be accepted");
        let pooled = eth_api.pool().get(&hash).expect("transaction should be in the pool");
        assert_eq!(pooled.transaction.chain_id(), Some(1));
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_chain_id() {
        let address = Address::random();
        let accounts = AddressMap::from_iter([(
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

        let accounts = AddressMap::from_iter([(
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
    async fn fill_frame_transaction_preserves_frame_limits() {
        let address = Address::random();
        let nonce = 42u64;
        let limits = FrameLimits { execution: 45_000, state: 7_000 };
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(nonce, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let eth_api = mock_frame_eth_api(accounts);

        let tx_req = TransactionRequest {
            from: Some(address),
            transaction_type: Some(0x06),
            frames: Some(vec![Frame { limits, ..Default::default() }.into()]),
            ..Default::default()
        };

        let filled = eth_api.fill_transaction(tx_req).await.expect("frame fill should succeed");
        let frame_tx = filled.tx.frame_transaction().expect("filled transaction should be a frame");

        assert_eq!(frame_tx.sender, address);
        assert_eq!(frame_tx.nonce, nonce);
        assert_eq!(frame_tx.nonce_keys, Some(vec![]));
        assert_eq!(frame_tx.chain_id, 1);
        assert_eq!(frame_tx.frames[0].limits, limits);
        assert!(frame_tx.signatures.is_empty());
        assert_eq!(filled.tx.gas_limit(), frame_tx.calculate_gas_limit());
    }

    #[tokio::test]
    async fn fill_frame_transaction_preserves_signature_placeholders() {
        let address = Address::random();
        let nonce = 42u64;
        let limits = FrameLimits { execution: 45_000, state: 7_000 };
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(nonce, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let eth_api = mock_frame_eth_api(accounts);

        let tx_req = TransactionRequest {
            from: Some(address),
            transaction_type: Some(0x06),
            signatures: Some(vec![alloy_eips::eip8141::FrameSignature {
                scheme: alloy_eips::eip8141::SignatureScheme::Secp256k1,
                ..Default::default()
            }]),
            frames: Some(vec![Frame { limits, ..Default::default() }.into()]),
            ..Default::default()
        };

        let filled = eth_api.fill_transaction(tx_req).await.expect("frame fill should succeed");
        let frame_tx = filled.tx.frame_transaction().expect("filled transaction should be a frame");

        assert_eq!(frame_tx.sender, address);
        assert_eq!(frame_tx.nonce, nonce);
        assert_eq!(frame_tx.nonce_keys, Some(vec![]));
        assert_eq!(frame_tx.chain_id, 1);
        assert_eq!(frame_tx.frames[0].limits, limits);
        assert_eq!(frame_tx.signatures.len(), 1);
        assert!(frame_tx.signatures[0].signature.is_empty());
        let mut completed = frame_tx.clone();
        completed.signatures[0].signature = Bytes::from(vec![0; 65]);
        let fee_rate = U256::from(2_000_000_000u64) + frame_tx.fees.max_priority_fee_per_gas;
        assert!(frame_tx.fees.max_fee >= U256::from(completed.gas_limit()) * fee_rate);
        let json = serde_json::to_value(&filled).unwrap();
        assert_eq!(json["tx"]["signatures"][0]["scheme"], "0x1");
        assert!(json["tx"]["signatures"][0].get("signature").is_none());
        assert_eq!(filled.tx.gas_limit(), frame_tx.calculate_gas_limit());
    }

    #[tokio::test]
    async fn fill_frame_transaction_estimates_missing_limits_and_preserves_zero() {
        let address = Address::random();
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);
        let api = mock_frame_eth_api(accounts);
        for state in [None, Some("0x0")] {
            let mut frame = serde_json::json!({"mode":"0x1", "flags":"0x3"});
            if let Some(state) = state {
                frame["stateGas"] = state.into();
            }
            let request: TransactionRequest = serde_json::from_value(serde_json::json!({
                "type":"0x6", "from": address, "frames":[frame], "signatures":[{"scheme":"0x1"}]
            }))
            .unwrap();
            let estimated = EthCall::estimate_gas_at(
                &api,
                request.clone(),
                BlockId::latest(),
                EvmOverrides::default(),
            )
            .await
            .unwrap();
            assert!(estimated > U256::ZERO);
            EthCall::call(&api, request.clone(), Some(BlockId::latest()), EvmOverrides::default())
                .await
                .unwrap();
            let filled = api.fill_transaction(request).await.unwrap();
            let transaction = filled.tx.frame_transaction().unwrap();
            assert!(transaction.frames[0].limits.execution > 0);
            assert_eq!(transaction.frames[0].limits.state, 0);
            assert!(transaction.frames[0].limits.execution < 100_000);
        }
    }

    #[tokio::test]
    async fn fill_frame_transaction_preserves_state_between_frames() {
        let sender = Address::random();
        let target = Address::random();
        let code = alloy_primitives::hex!("5f35156011575f546001146016575f5ffd5b60015f555b00");
        let api = mock_frame_eth_api(AddressMap::from_iter([
            (sender, ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64))),
            (
                target,
                ExtendedAccount::new(0, U256::ZERO).with_bytecode(Bytes::copy_from_slice(&code)),
            ),
        ]));
        let request = serde_json::from_value(serde_json::json!({
            "type":"0x6", "from":sender,
            "frames":[
                {"mode":"0x1", "flags":"0x3"},
                {"mode":"0x2", "target":target},
                {"mode":"0x2", "target":target, "data":"0x0000000000000000000000000000000000000000000000000000000000000001"}
            ],
            "signatures":[{"scheme":"0x1"}]
        })).unwrap();
        let filled = api.fill_transaction(request).await.unwrap();
        let frames = &filled.tx.frame_transaction().unwrap().frames;
        assert!(frames[1].limits.state > 0);
        assert_eq!(frames[2].limits.state, 0);
        assert!(frames[2].limits.execution > 0);
    }

    #[tokio::test]
    async fn fill_frame_limits_preserves_independent_block_budgets() {
        let sender = Address::repeat_byte(0x11);
        let target = Address::repeat_byte(0xbb);
        let block_limit = 1_000_000;
        let accounts = AddressMap::from_iter([(
            sender,
            ExtendedAccount::new(0, U256::MAX).with_bytecode(bytes!("60035f5faa00")),
        )]);
        let api = mock_frame_eth_api(accounts.clone());
        let request = TransactionRequest {
            from: Some(sender),
            frames: Some(vec![
                Frame {
                    mode: FrameMode::Verify,
                    flags: 3,
                    limits: FrameLimits { execution: 20_000, state: 0 },
                    ..Default::default()
                }
                .into(),
                Frame {
                    mode: FrameMode::Default,
                    target: target.into(),
                    limits: FrameLimits { execution: 500_000, state: 600_000 },
                    ..Default::default()
                }
                .into(),
                FrameRequest {
                    mode: FrameMode::Default,
                    target: target.into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let overrides = EvmOverrides {
            block: Some(Box::new(BlockOverrides {
                gas_limit: Some(block_limit),
                base_fee: Some(U256::ONE),
                ..Default::default()
            })),
            ..Default::default()
        };
        let available_permits = api.blocking_io_task_guard().available_permits();
        let estimated =
            EthCall::estimate_gas_at(&api, request.clone(), BlockId::latest(), overrides.clone())
                .await
                .unwrap();
        assert!(estimated > U256::from(block_limit));
        assert_eq!(api.blocking_io_task_guard().available_permits(), available_permits);
        for (execution, state, expected) in [
            (block_limit, 600_000, "frame transaction exceeds the execution gas cap"),
            (500_000, block_limit + 1, "frame transaction exceeds the state gas cap"),
        ] {
            let mut request = request.clone();
            let frame = &mut request.frames.as_mut().unwrap()[1];
            frame.execution_gas = Some(execution);
            frame.state_gas = Some(state);
            assert_eq!(
                EthCall::estimate_gas_at(&api, request, BlockId::latest(), overrides.clone())
                    .await
                    .unwrap_err()
                    .to_string(),
                expected
            );
        }
        let mut fee_capped = request.clone();
        fee_capped.max_fee = Some(U256::from(block_limit));
        assert_eq!(
            EthCall::estimate_gas_at(&api, fee_capped, BlockId::latest(), overrides.clone())
                .await
                .unwrap_err()
                .to_string(),
            "frame transaction exceeds the gas cap"
        );
        let rpc_capped =
            mock_eth_api_builder(accounts, ChainSpecBuilder::mainnet().bogota_activated().build())
                .gas_cap(GasCap(block_limit))
                .build();
        assert_eq!(
            EthCall::estimate_gas_at(
                &rpc_capped,
                request.clone(),
                BlockId::latest(),
                overrides.clone()
            )
            .await
            .unwrap_err()
            .to_string(),
            "frame transaction exceeds the gas cap"
        );
        api.blocking_io_task_guard().close();
        assert!(matches!(
            EthCall::estimate_gas_at(&api, request, BlockId::latest(), overrides).await,
            Err(EthApiError::InternalEthError)
        ));
    }

    #[tokio::test]
    async fn fill_frame_transaction_rejects_missing_limits_with_supplied_signature() {
        let address = Address::random();
        let api = mock_frame_eth_api(AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::MAX),
        )]));
        let request = serde_json::from_value(serde_json::json!({
            "type":"0x6", "from":address, "frames":[{"mode":"0x1", "flags":"0x3"}],
            "signatures":[{"scheme":"0x1", "signature": "0x01"}]
        }))
        .unwrap();
        assert_eq!(
            api.fill_transaction(request).await.unwrap_err().to_string(),
            "signed frame transactions must include nonceKeys before filling"
        );
    }

    #[tokio::test]
    async fn fill_frame_transaction_preserves_signed_nonce_keys() {
        let address = Address::random();
        let api = mock_frame_eth_api(AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::MAX),
        )]));
        let request: TransactionRequest = serde_json::from_value(serde_json::json!({
            "type":"0x6", "from":address, "chainId":"0x1", "nonce":"0x0",
            "maxFee":"0x0", "maxPriorityFeePerGas":"0x0",
            "frames":[{"mode":"0x1", "flags":"0x3", "executionGas":"0xafc8", "stateGas":"0x0"}],
            "signatures":[{"scheme":"0x1", "signature": Bytes::from(vec![1; 65])}]
        }))
        .unwrap();
        let error = api.fill_transaction(request.clone()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "signed frame transactions must include nonceKeys before filling"
        );
        for keys in [vec![], vec![U256::ZERO], vec![U256::from(1), U256::from(2)]] {
            let mut request = request.clone();
            request.nonce_keys = Some(keys.clone());
            let expected = request.clone().build_8141().unwrap();
            let filled = api.fill_transaction(request).await.unwrap();
            assert_eq!(filled.tx.frame_transaction().unwrap().nonce_keys, Some(keys));
            assert_eq!(filled.tx.frame_transaction().unwrap(), &expected);
        }
    }

    #[tokio::test]
    async fn fill_frame_transaction_does_not_replace_explicit_zero_execution() {
        let address = Address::random();
        let api = mock_frame_eth_api(AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::MAX),
        )]));
        let request = serde_json::from_value(serde_json::json!({
            "type":"0x6", "from":address, "frames":[{"mode":"0x1", "flags":"0x3", "executionGas":"0x0"}],
            "signatures":[{"scheme":"0x1"}]
        })).unwrap();
        assert!(matches!(
            api.fill_transaction(request).await.unwrap_err(),
            EthApiError::InvalidTransaction(
                reth_rpc_eth_types::RpcInvalidTransactionError::FrameTransaction(
                    evm2::registry::HandlerError::InvalidFrameTransaction(
                        evm2::registry::FrameTransactionError::VerifyFrameFailed
                    )
                )
            )
        ));
    }

    #[tokio::test]
    async fn estimate_frame_transaction_returns_derived_outer_limit() {
        let address = Address::random();
        let limits = FrameLimits { execution: 45_000, state: 7_000 };
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64))
                .with_bytecode(Bytes::from_static(&[0x60, 0x03, 0x5f, 0x5f, 0xaa, 0x00])),
        )]);
        let eth_api = mock_frame_eth_api(accounts);
        let tx_req = TransactionRequest {
            from: Some(address),
            transaction_type: Some(0x06),
            // The outer RPC field must not override the canonical reservation derived from the
            // frame execution and state limits.
            gas: Some(1),
            frames: Some(vec![Frame {
                mode: FrameMode::Verify,
                flags: 3,
                target: FrameAddress::from(address),
                limits,
                ..Default::default()
            }
            .into()]),
            ..Default::default()
        };
        let expected = eth_api
            .converter()
            .build_simulate_v1_transaction(TransactionRequest {
                nonce: Some(0),
                nonce_keys: Some(vec![]),
                gas: None,
                max_fee: Some(U256::MAX),
                max_priority_fee_per_gas: Some(0),
                signatures: Some(Vec::new()),
                ..tx_req.clone()
            })
            .expect("frame request should convert")
            .gas_limit();

        let estimated =
            EthCall::estimate_gas_at(&eth_api, tx_req, BlockId::latest(), EvmOverrides::default())
                .await
                .expect("frame gas estimation should succeed");

        assert_eq!(estimated, U256::from(expected));
    }

    #[tokio::test]
    async fn frame_pending_simulation_applies_nonce_transition_once() {
        let sender = Address::repeat_byte(0x11);
        for (timestamp, state_nonce, nonce, at) in [
            (1, 0, 1, BlockId::pending()),
            (13, 2, 2, BlockId::pending()),
            (13, 2, 2, BlockId::latest()),
        ] {
            let mut genesis = ChainSpecBuilder::mainnet().build().genesis.clone();
            genesis.timestamp = 0;
            genesis.config.bogota_time = Some(2);
            genesis.config.eip8253_accounts = Some(vec![sender]);
            let spec = ChainSpecBuilder::mainnet()
                .genesis(genesis)
                .amsterdam_activated()
                .with_bogota_at(2)
                .build();
            let api = mock_eth_api_builder(
                AddressMap::from_iter([(
                    sender,
                    ExtendedAccount::new(state_nonce, U256::from(10_000_000_000_000_000_000u64))
                        .with_bytecode(bytes!("60035f5faa00")),
                )]),
                spec,
            )
            .build();
            if timestamp > 1 {
                let header = Header {
                    number: 1,
                    timestamp,
                    gas_limit: 30_000_000,
                    base_fee_per_gas: Some(1_000_000_000),
                    excess_blob_gas: Some(0),
                    blob_gas_used: Some(0),
                    parent_beacon_block_root: Some(B256::ZERO),
                    slot_number: Some(1),
                    ..Default::default()
                };
                api.provider()
                    .add_block(header.hash_slow(), Block::new(header, Default::default()));
            }
            let transaction = TxEip8141 {
                chain_id: 1,
                sender,
                nonce,
                nonce_keys: Some(vec![]),
                frames: vec![Frame {
                    mode: FrameMode::Verify,
                    flags: 3,
                    limits: FrameLimits { execution: 20_000, state: 0 },
                    ..Default::default()
                }],
                fees: TransactionFees {
                    max_fee: U256::from(1_000_000_000_000_000_000u64),
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = EthCall::simulate_frame_transaction(
                &api,
                transaction.encoded_2718().into(),
                Some(at),
            )
            .await
            .unwrap();
            assert!(result.valid, "{:?}", result.violation);
            assert_eq!(result.frames.unwrap()[0].status, alloy_eips::eip8141::FrameStatus::Success);
            let request = <TransactionRequest as From<_>>::from(transaction);
            assert!(EthCall::call(&api, request.clone(), Some(at), EvmOverrides::default())
                .await
                .unwrap()
                .is_empty());
            assert!(
                EthCall::estimate_gas_at(&api, request, at, EvmOverrides::default()).await.unwrap() >
                    U256::ZERO
            );
        }
    }

    #[tokio::test]
    async fn frame_pending_deployment_observes_nonce_transition_collision() {
        let factory = Address::repeat_byte(0x22);
        let sender = factory.create(0);
        let mut genesis = ChainSpecBuilder::mainnet().build().genesis.clone();
        genesis.timestamp = 0;
        genesis.config.bogota_time = Some(2);
        genesis.config.eip8253_accounts = Some(vec![sender]);
        let spec = ChainSpecBuilder::mainnet()
            .genesis(genesis)
            .amsterdam_activated()
            .with_bogota_at(2)
            .build();
        let api = mock_eth_api_builder(
            AddressMap::from_iter([
                (
                    sender,
                    ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64))
                        .extend_storage([(B256::ZERO, U256::ONE)]),
                ),
                // CREATE installs the sender's approval runtime, or reverts if the address
                // collides.
                (
                    factory,
                    ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!(
                        "601060145f3960105f5ff015601057005b5f5ffd6006600a5f3960065ff360035f5faa00"
                    )),
                ),
            ]),
            spec,
        )
        .build();
        let transaction = TxEip8141 {
            chain_id: 1,
            sender,
            nonce_keys: Some(vec![]),
            frames: vec![
                Frame {
                    mode: FrameMode::Default,
                    target: factory.into(),
                    limits: FrameLimits { execution: 80_000, state: 500_000 },
                    ..Default::default()
                },
                Frame {
                    mode: FrameMode::Verify,
                    flags: 3,
                    limits: FrameLimits { execution: 20_000, state: 0 },
                    ..Default::default()
                },
            ],
            fees: TransactionFees {
                max_fee: U256::from(1_000_000_000_000_000_000u64),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = EthCall::simulate_frame_transaction(
            &api,
            transaction.encoded_2718().into(),
            Some(BlockId::pending()),
        )
        .await
        .unwrap();
        assert!(!result.valid);
        assert_eq!(result.violation.as_deref(), Some("EIP-8141 VERIFY frame failed"));
    }

    #[tokio::test]
    async fn simulate_frame_transaction_classifies_recent_root_prefix() {
        for slot in [0, 1] {
            let sender = Address::repeat_byte(0x11);
            let reference = RecentRootReference {
                source_id: B256::repeat_byte(1),
                slot,
                root: B256::repeat_byte(2),
            };
            let dependency = reference.dependency();
            let api = mock_frame_eth_api(AddressMap::from_iter([
                (
                    sender,
                    ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64))
                        .with_bytecode(bytes!("60035f5faa00")),
                ),
                (
                    RECENT_ROOT_ADDRESS,
                    ExtendedAccount::new(0, U256::ZERO)
                        .with_bytecode(RECENT_ROOT_CODE.clone())
                        .extend_storage([(
                            dependency.storage_key.into(),
                            dependency.entry_hash.into(),
                        )]),
                ),
            ]));
            let mut reference_data = reference.source_id.to_vec();
            reference_data.extend_from_slice(&reference.slot.to_be_bytes());
            reference_data.extend_from_slice(reference.root.as_slice());
            let transaction = TxEip8141 {
                chain_id: 1,
                sender,
                nonce_keys: Some(vec![]),
                frames: vec![
                    Frame {
                        mode: FrameMode::Verify,
                        target: RECENT_ROOT_ADDRESS.into(),
                        limits: FrameLimits { execution: 20_000, state: 0 },
                        data: reference_data.into(),
                        ..Default::default()
                    },
                    Frame {
                        mode: FrameMode::Verify,
                        flags: 3,
                        limits: FrameLimits { execution: 20_000, state: 0 },
                        ..Default::default()
                    },
                ],
                fees: TransactionFees {
                    max_fee: U256::from(1_000_000_000_000_000_000u64),
                    ..Default::default()
                },
                ..Default::default()
            };
            let available_permits = api.blocking_io_task_guard().available_permits();
            let result = EthCall::simulate_frame_transaction(
                &api,
                transaction.encoded_2718().into(),
                Some(BlockId::latest()),
            )
            .await
            .unwrap();
            assert!(result.valid, "{:?}", result.violation);
            assert_eq!(result.prefix_shape, Some(FrameSimulationPrefixShape::SelfVerify));
            assert_eq!(result.payer, Some(sender));
            assert_eq!(result.frames.unwrap().len(), transaction.frames.len());
            assert_eq!(api.blocking_io_task_guard().available_permits(), available_permits);
            api.blocking_io_task_guard().close();
            for raw in [transaction.encoded_2718().into(), Bytes::from_static(&[0x06])] {
                assert!(matches!(
                    EthCall::simulate_frame_transaction(&api, raw, Some(BlockId::latest()),).await,
                    Err(EthApiError::InternalEthError)
                ));
            }
        }
    }

    #[tokio::test]
    async fn simulate_frame_recent_root_uses_selected_pending_slot() {
        for at in [BlockId::latest(), BlockId::pending()] {
            for (parent_slot, reference_slot, valid) in [
                (RECENT_ROOT_LENGTH - 2, 0, true),
                (RECENT_ROOT_LENGTH - 1, 0, false),
                (7, 8, false),
            ] {
                let sender = Address::repeat_byte(0x11);
                let checker = Address::repeat_byte(0x22);
                let reference = RecentRootReference {
                    source_id: B256::repeat_byte(1),
                    slot: reference_slot,
                    root: B256::repeat_byte(2),
                };
                let dependency = reference.dependency();
                let api = mock_frame_eth_api(AddressMap::from_iter([
                    (
                        sender,
                        ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64))
                            .with_bytecode(bytes!("60035f5faa00")),
                    ),
                    (
                        checker,
                        // Revert unless SLOTNUM equals the first calldata word.
                        ExtendedAccount::new(0, U256::ZERO)
                            .with_bytecode(bytes!("4b5f3514600a575f5ffd5b00")),
                    ),
                    (
                        RECENT_ROOT_ADDRESS,
                        ExtendedAccount::new(0, U256::ZERO)
                            .with_bytecode(RECENT_ROOT_CODE.clone())
                            .extend_storage([(
                                dependency.storage_key.into(),
                                dependency.entry_hash.into(),
                            )]),
                    ),
                ]));
                let header = Header {
                    number: 1,
                    timestamp: 13,
                    gas_limit: 30_000_000,
                    base_fee_per_gas: Some(1_000_000_000),
                    excess_blob_gas: Some(0),
                    blob_gas_used: Some(0),
                    parent_beacon_block_root: Some(B256::ZERO),
                    slot_number: Some(parent_slot),
                    ..Default::default()
                };
                api.provider()
                    .add_block(header.hash_slow(), Block::new(header, Default::default()));
                let mut reference_data = reference.source_id.to_vec();
                reference_data.extend_from_slice(&reference.slot.to_be_bytes());
                reference_data.extend_from_slice(reference.root.as_slice());
                let transaction = TxEip8141 {
                    chain_id: 1,
                    sender,
                    nonce_keys: Some(vec![]),
                    frames: vec![
                        Frame {
                            mode: FrameMode::Verify,
                            target: RECENT_ROOT_ADDRESS.into(),
                            limits: FrameLimits { execution: 20_000, state: 0 },
                            data: reference_data.into(),
                            ..Default::default()
                        },
                        Frame {
                            mode: FrameMode::Verify,
                            flags: 3,
                            limits: FrameLimits { execution: 20_000, state: 0 },
                            ..Default::default()
                        },
                        Frame {
                            mode: FrameMode::Default,
                            target: checker.into(),
                            limits: FrameLimits { execution: 20_000, state: 0 },
                            data: U256::from(parent_slot + 1).to_be_bytes::<32>().into(),
                            ..Default::default()
                        },
                    ],
                    fees: TransactionFees {
                        max_fee: U256::from(1_000_000_000_000_000_000u64),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let result = EthCall::simulate_frame_transaction(
                    &api,
                    transaction.encoded_2718().into(),
                    Some(at),
                )
                .await
                .unwrap();
                assert_eq!(result.valid, valid, "at={at:?}, parent_slot={parent_slot}: {result:?}");
                if valid {
                    assert_eq!(result.violation, None);
                    assert_eq!(result.prefix_shape, Some(FrameSimulationPrefixShape::SelfVerify));
                    assert_eq!(result.payer, Some(sender));
                    let frames = result.frames.unwrap();
                    assert_eq!(frames.len(), transaction.frames.len());
                    assert!(frames.iter().all(|frame| frame.status == FrameStatus::Success));
                } else {
                    assert_eq!(
                        result.violation.as_deref(),
                        Some("recent root reference is outside the usable slot window")
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn test_fill_transaction_preserves_provided_fields() {
        let address = Address::random();
        let provided_nonce = 100u64;
        let provided_gas_limit = 50_000u64;

        let accounts = AddressMap::from_iter([(
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
        let accounts = AddressMap::from_iter([(address, ExtendedAccount::new(5, balance))]);

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
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // EIP-4844 blob transaction with versioned hashes but no blob fee
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            sidecar: Some(BlobTransactionSidecarVariant::from(zero_blob_sidecar())),
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
        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        let provided_blob_fee = 5000000u128;

        // EIP-4844 blob transaction with blob fee already set
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(3), // EIP-4844
            sidecar: Some(BlobTransactionSidecarVariant::from(zero_blob_sidecar())),
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
        let accounts = AddressMap::from_iter([(
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
