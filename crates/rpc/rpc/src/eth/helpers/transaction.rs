//! Contains RPC handler implementations specific to transactions

use crate::EthApi;
use alloy_consensus::BlobTransactionValidationError;
use alloy_eips::{eip7594::BlobTransactionSidecarVariant, BlockId, Typed2718};
use alloy_primitives::{hex, Bytes, B256};
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
    batcher::{IngressError, IngressOutcome, IngressPermit},
    error::Eip4844PoolTransactionError,
    AddedTransactionOutcome, EthBlobTransactionSidecar, EthPoolTransaction, PoolTransaction,
    PoolTx,
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

    async fn send_raw_transaction(&self, bytes: Bytes) -> Result<B256, Self::Error> {
        let ingress = self.inner.transaction_batcher();
        let origin = reth_transaction_pool::TransactionOrigin::Local;
        if self.raw_tx_forwarder().is_none() && !self.inner.force_blob_sidecar_upcasting() {
            let notifications = self.inner.raw_tx_sender().clone();
            let raw = bytes.clone();
            let result = ingress.submit_raw(origin, bytes, move || {
                let _ = notifications.send(raw);
            })?;
            return match result.await.map_err(|_| IngressError::Closed)?? {
                IngressOutcome::Inserted(outcome) => Ok(outcome.hash),
                IngressOutcome::Recovered(..) => unreachable!("insertion request"),
            }
        }
        // Forwarding and optional sidecar conversion await other services. Keep their work
        // admitted, but release the recovery worker while those services are awaited.
        match ingress.recover_raw(bytes.clone())?.await.map_err(|_| IngressError::Closed)?? {
            IngressOutcome::Recovered(transaction, permit) => {
                self.send_admitted_pool_transaction(
                    origin,
                    WithEncoded::new(bytes, transaction),
                    Some(permit),
                )
                .await
            }
            IngressOutcome::Inserted(..) => unreachable!("recovery request"),
        }
    }

    async fn send_pool_transaction(
        &self,
        origin: reth_transaction_pool::TransactionOrigin,
        tx: WithEncoded<PoolTx<Self::Pool>>,
    ) -> Result<B256, Self::Error> {
        self.send_admitted_pool_transaction(origin, tx, None).await
    }
}

impl<N, Rpc> EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
    async fn send_admitted_pool_transaction(
        &self,
        origin: reth_transaction_pool::TransactionOrigin,
        tx: WithEncoded<PoolTx<N::Pool>>,
        permit: Option<IngressPermit>,
    ) -> Result<B256, EthApiError> {
        let permit = match permit {
            Some(permit) => permit,
            None => self
                .inner
                .transaction_batcher()
                .reserve_rpc(tx.value().ingress_size().saturating_add(tx.encoded_bytes().len()))?,
        };
        if self.raw_tx_forwarder().is_none() &&
            !(self.inner.force_blob_sidecar_upcasting() && tx.value().is_eip4844())
        {
            return self.prepare_pool_transaction(origin, tx, permit).await
        }
        // Conversion can outlive a canceled RPC future. Keep its input and reservation together,
        // including for already-recovered submissions that bypass raw recovery.
        let this = self.clone();
        tokio::spawn(async move { this.prepare_pool_transaction(origin, tx, permit).await })
            .await
            .map_err(|error| EthApiError::Internal(reth_errors::RethError::other(error)))?
    }

    async fn prepare_pool_transaction(
        &self,
        origin: reth_transaction_pool::TransactionOrigin,
        tx: WithEncoded<PoolTx<N::Pool>>,
        mut permit: IngressPermit,
    ) -> Result<B256, EthApiError> {
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
                        let proofs = sidecar
                            .blobs
                            .len()
                            .saturating_mul(alloy_eips::eip7594::CELLS_PER_EXT_BLOB)
                            .saturating_mul(alloy_eips::eip4844::BYTES_PER_PROOF);
                        permit.grow(
                            pool_transaction
                                .ingress_size()
                                .saturating_add(tx.len())
                                .saturating_add(sidecar.size())
                                .saturating_add(proofs),
                        )?;

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
            permit.grow(
                pool_transaction
                    .ingress_size()
                    .saturating_add(tx.len())
                    .saturating_add(tx.len().saturating_mul(2).saturating_add(2)),
            )?;

            let rlp_hex = hex::encode_prefixed(&tx);

            // broadcast raw transaction to subscribers if there is any.
            self.broadcast_raw_transaction(tx);

            let hash =
                client.request("eth_sendRawTransaction", (rlp_hex,)).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                }).map_err(EthApiError::other)?;

            // Retain tx in local tx pool after forwarding, for local RPC usage.
            let _ =
                self.inner.add_admitted_pool_transaction(origin, pool_transaction, permit).await;

            return Ok(hash);
        }

        // broadcast raw transaction to subscribers if there is any.
        self.broadcast_raw_transaction(tx);

        let AddedTransactionOutcome { hash, .. } =
            self.inner.add_admitted_pool_transaction(origin, pool_transaction, permit).await?;

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
    use alloy_consensus::{
        BlobTransactionSidecar, Block, Header, SidecarBuilder, SimpleCoder, Transaction,
    };
    use alloy_primitives::{map::AddressMap, Address, Bytes, U256};
    use alloy_rpc_types_eth::request::TransactionRequest;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
    use reth_transaction_pool::{
        test_utils::{testing_pool, TestPool},
        TransactionOrigin, TransactionPool,
    };

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

        EthApi::builder(mock_provider, pool, NoopNetwork::default(), evm_config)
            .send_raw_transaction_sync_timeout(send_raw_transaction_sync_timeout)
            .build()
    }

    fn raw_transfer_tx() -> Bytes {
        // https://etherscan.io/tx/0xa694b71e6c128a2ed8e2e0f6770bddbe52e3bb8f10e8472f9a79ab81497a8b5d
        Bytes::from(hex!(
            "02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3"
        ))
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
        assert_eq!(pool.get(&tx_1_result).unwrap().origin, TransactionOrigin::Local);
        assert_eq!(pool.get(&tx_2_result).unwrap().origin, TransactionOrigin::Local);
    }

    #[tokio::test]
    async fn canceled_paused_rpc_does_not_retain_api() {
        let api = mock_eth_api(Default::default());
        let weak = std::sync::Arc::downgrade(&api.inner);
        let pause = api.inner.transaction_batcher().pause_handle().pause();
        let mut submission = Box::pin(api.send_raw_transaction(raw_transfer_tx()));
        assert!(futures::poll!(&mut submission).is_pending());
        drop(submission);
        drop(api);
        assert!(weak.upgrade().is_none());
        drop(pause);
    }

    #[tokio::test]
    async fn raw_ingress_preserves_errors_and_subscription_delivery() {
        let api = mock_eth_api(Default::default());
        let mut raw = api.inner.subscribe_to_raw_transactions();
        assert!(matches!(
            api.send_raw_transaction(Bytes::new()).await,
            Err(EthApiError::EmptyRawTransactionData)
        ));
        assert!(matches!(
            api.send_raw_transaction(Bytes::from_static(&[0xff])).await,
            Err(EthApiError::FailedToDecodeSignedTransaction)
        ));
        assert!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
        let transaction = raw_transfer_tx();
        api.send_raw_transaction(transaction.clone()).await.unwrap();
        assert_eq!(raw.recv().await.unwrap(), transaction);
        assert!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn raw_ingress_forwarding_returns_remote_hash_and_retains_local_transaction() {
        let server =
            jsonrpsee::server::ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let expected_raw = hex::encode_prefixed(raw_transfer_tx());
        let remote_hash = B256::repeat_byte(7);
        let mut module = jsonrpsee::RpcModule::new(());
        module
            .register_method("eth_sendRawTransaction", move |params, _, _| {
                assert_eq!(params.one::<String>().unwrap(), expected_raw);
                Ok::<_, jsonrpsee::types::ErrorObjectOwned>(remote_hash)
            })
            .unwrap();
        let server = server.start(module);
        let provider = MockEthProvider::default();
        let evm = EthEvmConfig::new(provider.chain_spec());
        let api = EthApi::builder(provider, testing_pool(), NoopNetwork::default(), evm)
            .raw_tx_forwarder(reth_rpc_eth_types::ForwardConfig {
                tx_forwarder: Some(format!("http://{address}").parse().unwrap()),
            })
            .build();
        let mut raw = api.inner.subscribe_to_raw_transactions();
        assert_eq!(api.send_raw_transaction(raw_transfer_tx()).await.unwrap(), remote_hash);
        assert_eq!(api.pool().len(), 1);
        assert_eq!(raw.recv().await.unwrap(), raw_transfer_tx());
        server.stop().unwrap();
    }

    #[tokio::test]
    async fn forwarding_reserves_encoding_before_contacting_remote() {
        use reth_transaction_pool::{BatchTxConfig, BatchTxProcessor};
        let raw = raw_transfer_tx();
        let tx = PoolTx::<TestPool>::recover_raw_transaction(&raw).unwrap();
        let bytes = tx.ingress_size() + raw.len();
        let pool = testing_pool();
        let (processor, ingress) = BatchTxProcessor::with_pool(
            pool.clone(),
            BatchTxConfig { max_bytes: bytes, ..Default::default() },
            None,
        );
        tokio::spawn(processor);
        let provider = MockEthProvider::default();
        let evm = EthEvmConfig::new(provider.chain_spec());
        let api = EthApi::builder(provider, pool, NoopNetwork::default(), evm)
            .transaction_batcher(Some(ingress.clone()))
            .raw_tx_forwarder(reth_rpc_eth_types::ForwardConfig {
                tx_forwarder: Some("http://127.0.0.1:1".parse().unwrap()),
            })
            .build();
        for recovered in [false, true] {
            let result = if recovered {
                api.send_pool_transaction(
                    TransactionOrigin::Local,
                    WithEncoded::new(raw.clone(), tx.clone()),
                )
                .await
            } else {
                api.send_raw_transaction(raw.clone()).await
            };
            assert!(matches!(result,
                Err(EthApiError::PoolError(RpcPoolError::Other(error)))
                    if matches!(error.downcast_ref::<IngressError>(), Some(IngressError::Full))
            ));
            assert!(ingress.reserve_rpc(bytes).is_ok(), "failed preparation releases all bytes");
        }
    }

    #[tokio::test]
    async fn canceled_rpc_preparation_retains_admission_until_completion() {
        use reth_transaction_pool::{BatchTxConfig, BatchTxProcessor};
        use std::sync::Arc;
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let server =
            jsonrpsee::server::ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut module = jsonrpsee::RpcModule::new((started.clone(), release.clone()));
        module
            .register_async_method("eth_sendRawTransaction", |_, signals, _| async move {
                signals.0.notify_one();
                signals.1.notified().await;
                Ok::<_, jsonrpsee::types::ErrorObjectOwned>(B256::repeat_byte(7))
            })
            .unwrap();
        let server = server.start(module);
        let pool = testing_pool();
        let (processor, ingress) = BatchTxProcessor::with_pool(
            pool.clone(),
            BatchTxConfig { max_transactions: 1, ..Default::default() },
            None,
        );
        tokio::spawn(processor);
        let provider = MockEthProvider::default();
        let evm = EthEvmConfig::new(provider.chain_spec());
        let api = EthApi::builder(provider, pool.clone(), NoopNetwork::default(), evm)
            .transaction_batcher(Some(ingress.clone()))
            .raw_tx_forwarder(reth_rpc_eth_types::ForwardConfig {
                tx_forwarder: Some(format!("http://{address}").parse().unwrap()),
            })
            .build();
        for recovered in [false, true] {
            let api = api.clone();
            let request = tokio::spawn(async move {
                let raw = raw_transfer_tx();
                if recovered {
                    let tx = PoolTx::<TestPool>::recover_raw_transaction(&raw).unwrap();
                    api.send_pool_transaction(TransactionOrigin::Local, WithEncoded::new(raw, tx))
                        .await
                } else {
                    api.send_raw_transaction(raw).await
                }
            });
            tokio::time::timeout(Duration::from_secs(2), started.notified()).await.unwrap();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert!(matches!(ingress.recover_raw(Bytes::new()), Err(IngressError::Full)));
            release.notify_one();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    match ingress.recover_raw(Bytes::new()) {
                        Ok(response) => {
                            let _ = response.await;
                            break;
                        }
                        Err(IngressError::Full) => tokio::task::yield_now().await,
                        Err(error) => panic!("unexpected ingress error: {error}"),
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(pool.len(), 1);
        }
        server.stop().unwrap();
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

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");

        // EIP-4844 blob transaction with versioned hashes but no blob fee
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            sidecar: Some(BlobTransactionSidecarVariant::from(
                builder.build::<BlobTransactionSidecar>().unwrap(),
            )),
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

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");

        // EIP-4844 blob transaction with blob fee already set
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(3), // EIP-4844
            sidecar: Some(BlobTransactionSidecarVariant::from(
                builder.build::<BlobTransactionSidecar>().unwrap(),
            )),
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
