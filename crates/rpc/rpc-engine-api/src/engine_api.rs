use crate::{
    bal_cache::BalCache,
    bal_store::{BalStore, BalStoreError},
    capabilities::EngineCapabilities,
    metrics::EngineApiMetrics,
    EngineApiError, EngineApiResult,
};
use alloy_eips::{
    eip1898::BlockHashOrNumber,
    eip4844::{BlobAndProofV1, BlobAndProofV2},
    eip4895::Withdrawals,
    eip7685::RequestsOrHash,
    BlockNumHash,
};
use alloy_primitives::{BlockHash, BlockNumber, Bytes, B256, U64};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ClientVersionV1, ExecutionData, ExecutionPayloadBodiesV1,
    ExecutionPayloadBodiesV2, ExecutionPayloadBodyV1, ExecutionPayloadBodyV2,
    ExecutionPayloadInputV2, ExecutionPayloadSidecar, ExecutionPayloadV1, ExecutionPayloadV3,
    ExecutionPayloadV4, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
    PraguePayloadFields,
};
use async_trait::async_trait;
use jsonrpsee_core::{server::RpcModule, RpcResult};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::{ConsensusEngineHandle, EngineApiValidator, EngineTypes};
use reth_network_api::NetworkInfo;
use reth_payload_builder::PayloadStore;
use reth_payload_primitives::{
    validate_payload_timestamp, EngineApiMessageVersion, ExecutionPayload, MessageValidationKind,
    PayloadOrAttributes, PayloadTypes,
};
use reth_primitives_traits::{Block, BlockBody};
use reth_rpc_api::{EngineApiServer, IntoEngineApiRpcModule};
use reth_storage_api::{BlockReader, HeaderProvider, StateProviderFactory};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{
    sync::Arc,
    time::{Instant, SystemTime},
};
use tokio::sync::oneshot;
use tracing::{debug, trace, warn};

/// The Engine API response sender.
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// The upper limit for payload bodies request.
const MAX_PAYLOAD_BODIES_LIMIT: u64 = 1024;

/// The upper limit for blobs in `engine_getBlobsVx`.
const MAX_BLOB_LIMIT: usize = 128;

#[derive(Clone)]
struct BalProvider {
    store: Arc<dyn BalStore>,
    cache: BalCache,
}

impl BalProvider {
    fn new(store: Arc<dyn BalStore>, cache: BalCache) -> Self {
        Self { store, cache }
    }

    const fn cache(&self) -> &BalCache {
        &self.cache
    }

    // Persist first: store is the source of truth. We only populate the in-memory cache if
    // durability succeeds, so cache visibility cannot outlive failed persistence.
    // `Bytes` is consumed by each insert call, so we clone once for store and move the original
    // into cache.
    fn cache_bal(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> Result<(), BalStoreError> {
        self.store.insert(block_hash, block_number, bal.clone())?;
        self.cache.insert(block_hash, block_number, bal);
        Ok(())
    }

    // Intent: serve BAL hash queries with cache-first latency while preserving
    // request-order semantics and filling only missing entries from durable storage.
    // Cache-first lookup: keep request order and fill only cache misses from durable storage.
    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
        metrics: &crate::metrics::BalQueryMetrics,
    ) -> Vec<Option<Bytes>> {
        let mut results = self.cache.get_by_hashes(block_hashes);

        // Collect missing positions so store fallback can patch holes in-place.
        let mut missing_hashes = Vec::new();
        let mut missing_indices = Vec::new();
        for (idx, result) in results.iter().enumerate() {
            if result.is_none() {
                missing_indices.push(idx);
                missing_hashes.push(block_hashes[idx]);
            }
        }

        if missing_hashes.is_empty() {
            return results;
        }

        metrics.store_hash_fallback_requests.increment(1);
        match self.store.get_by_hashes(&missing_hashes) {
            Ok(store_results) => {
                let mut recovered = 0_u64;
                let mut still_missing = 0_u64;

                for (missing_idx, store_result) in
                    missing_indices.into_iter().zip(store_results.into_iter())
                {
                    if let Some(value) = store_result {
                        results[missing_idx] = Some(value);
                        recovered += 1;
                    } else {
                        still_missing += 1;
                    }
                }

                if recovered > 0 {
                    metrics.store_hash_fallback_hits.increment(recovered);
                }
                if still_missing > 0 {
                    metrics.store_hash_fallback_misses.increment(still_missing);
                }
            }
            Err(err) => {
                metrics.store_hash_fallback_errors.increment(1);
                warn!(target: "rpc::engine", ?err, "Failed to retrieve BALs by hash from BAL store");
            }
        }

        results
    }

    // Intent: serve contiguous BAL range queries with cache-first latency and
    // append-only fallback from store, so the returned slice remains ordered and gap-safe.
    // Cache range reads are contiguous and stop at the first gap.
    fn get_by_range(
        &self,
        start: BlockNumber,
        count: u64,
        metrics: &crate::metrics::BalQueryMetrics,
    ) -> Vec<Bytes> {
        let mut cache_results = self.cache.get_by_range(start, count);
        if cache_results.len() as u64 == count {
            return cache_results;
        }

        // Only the missing suffix can be queried from store, which avoids re-reading cached prefix.
        let cached_len = cache_results.len() as u64;
        let missing_start = start.saturating_add(cached_len);
        let missing_count = count - cached_len;

        metrics.store_range_fallback_requests.increment(1);
        match self.store.get_by_range(missing_start, missing_count) {
            Ok(mut store_results) => {
                let recovered = store_results.len() as u64;
                if recovered > 0 {
                    metrics.store_range_fallback_hits.increment(recovered);
                }
                if recovered < missing_count {
                    metrics.store_range_fallback_misses.increment(missing_count - recovered);
                }
                cache_results.append(&mut store_results);
                cache_results
            }
            Err(err) => {
                metrics.store_range_fallback_errors.increment(1);
                warn!(target: "rpc::engine", ?err, "Failed to retrieve BALs by range from BAL store");
                cache_results
            }
        }
    }
}

/// The Engine API implementation that grants the Consensus layer access to data and
/// functions in the Execution layer that are crucial for the consensus process.
///
/// This type is generic over [`EngineTypes`] and intended to be used as the entrypoint for engine
/// API processing. It can be reused by other non L1 engine APIs that deviate from the L1 spec but
/// are still follow the engine API model.
///
/// ## Implementers
///
/// Implementing support for an engine API jsonrpsee RPC handler is done by defining the engine API
/// server trait and implementing it on a type that can either wrap this [`EngineApi`] type or
/// use a custom [`EngineTypes`] implementation if it mirrors ethereum's versioned engine API
/// endpoints (e.g. opstack).
/// See also [`EngineApiServer`] implementation for this type which is the
/// L1 implementation.
pub struct EngineApi<Provider, PayloadT: PayloadTypes, Pool, Validator, ChainSpec> {
    inner: Arc<EngineApiInner<Provider, PayloadT, Pool, Validator, ChainSpec>>,
}

impl<Provider, PayloadT: PayloadTypes, Pool, Validator, ChainSpec>
    EngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>
{
    /// Returns the configured chainspec.
    pub fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.inner.chain_spec
    }
}

impl<Provider, PayloadT, Pool, Validator, ChainSpec>
    EngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    PayloadT: PayloadTypes,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<PayloadT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    /// Create new instance of [`EngineApi`].
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: ConsensusEngineHandle<PayloadT>,
        payload_store: PayloadStore<PayloadT>,
        tx_pool: Pool,
        task_spawner: Box<dyn TaskSpawner>,
        client: ClientVersionV1,
        capabilities: EngineCapabilities,
        validator: Validator,
        accept_execution_requests_hash: bool,
        network: impl NetworkInfo + 'static,
        bal_store: Arc<dyn BalStore>,
    ) -> Self {
        Self::with_bal_store_and_cache(
            provider,
            chain_spec,
            beacon_consensus,
            payload_store,
            tx_pool,
            task_spawner,
            client,
            capabilities,
            validator,
            accept_execution_requests_hash,
            network,
            bal_store,
            BalCache::new(),
        )
    }

    /// Create new instance of [`EngineApi`] with a custom BAL cache.
    #[expect(clippy::too_many_arguments)]
    pub fn with_bal_cache(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: ConsensusEngineHandle<PayloadT>,
        payload_store: PayloadStore<PayloadT>,
        tx_pool: Pool,
        task_spawner: Box<dyn TaskSpawner>,
        client: ClientVersionV1,
        capabilities: EngineCapabilities,
        validator: Validator,
        accept_execution_requests_hash: bool,
        network: impl NetworkInfo + 'static,
        bal_cache: BalCache,
    ) -> Self {
        Self::with_bal_store_and_cache(
            provider,
            chain_spec,
            beacon_consensus,
            payload_store,
            tx_pool,
            task_spawner,
            client,
            capabilities,
            validator,
            accept_execution_requests_hash,
            network,
            Arc::new(bal_cache.clone()),
            bal_cache,
        )
    }

    /// Create new instance of [`EngineApi`] with a custom BAL store.
    #[expect(clippy::too_many_arguments)]
    pub fn with_bal_store(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: ConsensusEngineHandle<PayloadT>,
        payload_store: PayloadStore<PayloadT>,
        tx_pool: Pool,
        task_spawner: Box<dyn TaskSpawner>,
        client: ClientVersionV1,
        capabilities: EngineCapabilities,
        validator: Validator,
        accept_execution_requests_hash: bool,
        network: impl NetworkInfo + 'static,
        bal_store: Arc<dyn BalStore>,
    ) -> Self {
        Self::new(
            provider,
            chain_spec,
            beacon_consensus,
            payload_store,
            tx_pool,
            task_spawner,
            client,
            capabilities,
            validator,
            accept_execution_requests_hash,
            network,
            bal_store,
        )
    }

    /// Internal constructor that wires explicit BAL store and cache layers.
    #[expect(clippy::too_many_arguments)]
    fn with_bal_store_and_cache(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: ConsensusEngineHandle<PayloadT>,
        payload_store: PayloadStore<PayloadT>,
        tx_pool: Pool,
        task_spawner: Box<dyn TaskSpawner>,
        client: ClientVersionV1,
        capabilities: EngineCapabilities,
        validator: Validator,
        accept_execution_requests_hash: bool,
        network: impl NetworkInfo + 'static,
        bal_store: Arc<dyn BalStore>,
        bal_cache: BalCache,
    ) -> Self {
        let is_syncing = Arc::new(move || network.is_syncing());
        let bal_provider = BalProvider::new(bal_store, bal_cache);
        let inner = Arc::new(EngineApiInner {
            provider,
            chain_spec,
            beacon_consensus,
            payload_store,
            task_spawner,
            metrics: EngineApiMetrics::default(),
            client,
            capabilities,
            tx_pool,
            validator,
            accept_execution_requests_hash,
            is_syncing,
            bal_provider,
        });
        Self { inner }
    }

    /// Returns a reference to the BAL cache.
    pub fn bal_cache(&self) -> &BalCache {
        self.inner.bal_provider.cache()
    }

    /// Caches the BAL if the status is valid.
    fn maybe_cache_bal(&self, num_hash: BlockNumHash, bal: Option<Bytes>, status: &PayloadStatus) {
        if status.is_valid() &&
            let Some(bal) = bal &&
            let Err(err) = self.inner.bal_provider.cache_bal(num_hash.hash, num_hash.number, bal)
        {
            warn!(
                target: "rpc::engine",
                ?err,
                block_hash = ?num_hash.hash,
                block_number = num_hash.number,
                "Failed to persist BAL into BAL store"
            );
        }
    }

    /// Fetches the client version.
    pub fn get_client_version_v1(
        &self,
        _client: ClientVersionV1,
    ) -> EngineApiResult<Vec<ClientVersionV1>> {
        Ok(vec![self.inner.client.clone()])
    }

    /// Fetches the timestamp of the payload with the given id.
    async fn get_payload_timestamp(&self, payload_id: PayloadId) -> EngineApiResult<u64> {
        Ok(self
            .inner
            .payload_store
            .payload_timestamp(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)??)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    pub async fn new_payload_v1(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> EngineApiResult<PayloadStatus> {
        let payload_or_attrs = PayloadOrAttributes::<
            '_,
            PayloadT::ExecutionData,
            PayloadT::PayloadAttributes,
        >::from_execution_payload(&payload);

        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V1, payload_or_attrs)?;

        let num_hash = payload.num_hash();
        let bal = payload.block_access_list().cloned();
        let status = self.inner.beacon_consensus.new_payload(payload).await?;
        self.maybe_cache_bal(num_hash, bal, &status);
        Ok(status)
    }

    /// Metered version of `new_payload_v1`.
    pub async fn new_payload_v1_metered(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> EngineApiResult<PayloadStatus> {
        let start = Instant::now();
        let res = Self::new_payload_v1(self, payload).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v1.record(elapsed);
        res
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    pub async fn new_payload_v2(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> EngineApiResult<PayloadStatus> {
        let payload_or_attrs = PayloadOrAttributes::<
            '_,
            PayloadT::ExecutionData,
            PayloadT::PayloadAttributes,
        >::from_execution_payload(&payload);
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V2, payload_or_attrs)?;

        let num_hash = payload.num_hash();
        let bal = payload.block_access_list().cloned();
        let status = self.inner.beacon_consensus.new_payload(payload).await?;
        self.maybe_cache_bal(num_hash, bal, &status);
        Ok(status)
    }

    /// Metered version of `new_payload_v2`.
    pub async fn new_payload_v2_metered(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> EngineApiResult<PayloadStatus> {
        let start = Instant::now();
        let res = Self::new_payload_v2(self, payload).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v2.record(elapsed);
        res
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
    pub async fn new_payload_v3(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> EngineApiResult<PayloadStatus> {
        let payload_or_attrs = PayloadOrAttributes::<
            '_,
            PayloadT::ExecutionData,
            PayloadT::PayloadAttributes,
        >::from_execution_payload(&payload);
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V3, payload_or_attrs)?;

        let num_hash = payload.num_hash();
        let bal = payload.block_access_list().cloned();
        let status = self.inner.beacon_consensus.new_payload(payload).await?;
        self.maybe_cache_bal(num_hash, bal, &status);
        Ok(status)
    }

    /// Metrics version of `new_payload_v3`
    pub async fn new_payload_v3_metered(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> RpcResult<PayloadStatus> {
        let start = Instant::now();

        let res = Self::new_payload_v3(self, payload).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v3.record(elapsed);
        Ok(res?)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/7907424db935b93c2fe6a3c0faab943adebe8557/src/engine/prague.md#engine_newpayloadv4>
    pub async fn new_payload_v4(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> EngineApiResult<PayloadStatus> {
        let payload_or_attrs = PayloadOrAttributes::<
            '_,
            PayloadT::ExecutionData,
            PayloadT::PayloadAttributes,
        >::from_execution_payload(&payload);
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V4, payload_or_attrs)?;

        let num_hash = payload.num_hash();
        let bal = payload.block_access_list().cloned();
        let status = self.inner.beacon_consensus.new_payload(payload).await?;
        self.maybe_cache_bal(num_hash, bal, &status);
        Ok(status)
    }

    /// Metrics version of `new_payload_v4`
    pub async fn new_payload_v4_metered(
        &self,
        payload: PayloadT::ExecutionData,
    ) -> RpcResult<PayloadStatus> {
        let start = Instant::now();
        let res = Self::new_payload_v4(self, payload).await;

        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v4.record(elapsed);
        Ok(res?)
    }

    /// Returns whether the engine accepts execution requests hash.
    pub fn accept_execution_requests_hash(&self) -> bool {
        self.inner.accept_execution_requests_hash
    }
}

impl<Provider, EngineT, Pool, Validator, ChainSpec>
    EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    EngineT: EngineTypes,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EngineT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    /// Sends a message to the beacon consensus engine to update the fork choice _without_
    /// withdrawals.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_forkchoiceUpdatedV1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    pub async fn fork_choice_updated_v1(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        self.validate_and_execute_forkchoice(EngineApiMessageVersion::V1, state, payload_attrs)
            .await
    }

    /// Metrics version of `fork_choice_updated_v1`
    pub async fn fork_choice_updated_v1_metered(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        let start = Instant::now();
        let res = Self::fork_choice_updated_v1(self, state, payload_attrs).await;
        self.inner.metrics.latency.fork_choice_updated_v1.record(start.elapsed());
        res
    }

    /// Sends a message to the beacon consensus engine to update the fork choice _with_ withdrawals,
    /// but only _after_ shanghai.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    pub async fn fork_choice_updated_v2(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        self.validate_and_execute_forkchoice(EngineApiMessageVersion::V2, state, payload_attrs)
            .await
    }

    /// Metrics version of `fork_choice_updated_v2`
    pub async fn fork_choice_updated_v2_metered(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        let start = Instant::now();
        let res = Self::fork_choice_updated_v2(self, state, payload_attrs).await;
        self.inner.metrics.latency.fork_choice_updated_v2.record(start.elapsed());
        res
    }

    /// Sends a message to the beacon consensus engine to update the fork choice _with_ withdrawals,
    /// but only _after_ cancun.
    ///
    /// See also  <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_forkchoiceupdatedv3>
    pub async fn fork_choice_updated_v3(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        self.validate_and_execute_forkchoice(EngineApiMessageVersion::V3, state, payload_attrs)
            .await
    }

    /// Metrics version of `fork_choice_updated_v3`
    pub async fn fork_choice_updated_v3_metered(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        let start = Instant::now();
        let res = Self::fork_choice_updated_v3(self, state, payload_attrs).await;
        self.inner.metrics.latency.fork_choice_updated_v3.record(start.elapsed());
        res
    }

    /// Helper function for retrieving the build payload by id.
    async fn get_built_payload(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::BuiltPayload> {
        self.inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map_err(|_| EngineApiError::UnknownPayload)
    }

    /// Helper function for validating the payload timestamp and retrieving & converting the payload
    /// into desired envelope.
    async fn get_payload_inner<R>(
        &self,
        payload_id: PayloadId,
        version: EngineApiMessageVersion,
    ) -> EngineApiResult<R>
    where
        EngineT::BuiltPayload: TryInto<R>,
    {
        // Validate timestamp according to engine rules
        // Enforces Osaka restrictions on `getPayloadV4`.
        let timestamp = self.get_payload_timestamp(payload_id).await?;
        validate_payload_timestamp(
            &self.inner.chain_spec,
            version,
            timestamp,
            MessageValidationKind::GetPayload,
        )?;

        // Now resolve the payload
        self.get_built_payload(payload_id).await?.try_into().map_err(|_| {
            warn!(?version, "could not transform built payload");
            EngineApiError::UnknownPayload
        })
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV1> {
        self.get_built_payload(payload_id).await?.try_into().map_err(|_| {
            warn!(version = ?EngineApiMessageVersion::V1, "could not transform built payload");
            EngineApiError::UnknownPayload
        })
    }

    /// Metrics version of `get_payload_v1`
    pub async fn get_payload_v1_metered(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV1> {
        let start = Instant::now();
        let res = Self::get_payload_v1(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v1.record(start.elapsed());
        res
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_getpayloadv2>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV2> {
        self.get_payload_inner(payload_id, EngineApiMessageVersion::V2).await
    }

    /// Metrics version of `get_payload_v2`
    pub async fn get_payload_v2_metered(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV2> {
        let start = Instant::now();
        let res = Self::get_payload_v2(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v2.record(start.elapsed());
        res
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_getpayloadv3>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV3> {
        self.get_payload_inner(payload_id, EngineApiMessageVersion::V3).await
    }

    /// Metrics version of `get_payload_v3`
    pub async fn get_payload_v3_metered(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV3> {
        let start = Instant::now();
        let res = Self::get_payload_v3(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v3.record(start.elapsed());
        res
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/7907424db935b93c2fe6a3c0faab943adebe8557/src/engine/prague.md#engine_getpayloadv4>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV4> {
        self.get_payload_inner(payload_id, EngineApiMessageVersion::V4).await
    }

    /// Metrics version of `get_payload_v4`
    pub async fn get_payload_v4_metered(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV4> {
        let start = Instant::now();
        let res = Self::get_payload_v4(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v4.record(start.elapsed());
        res
    }

    /// Handler for `engine_getPayloadV5`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/15399c2e2f16a5f800bf3f285640357e2c245ad9/src/engine/osaka.md#engine_getpayloadv5>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v5(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV5> {
        self.get_payload_inner(payload_id, EngineApiMessageVersion::V5).await
    }

    /// Metrics version of `get_payload_v5`
    pub async fn get_payload_v5_metered(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadEnvelopeV5> {
        let start = Instant::now();
        let res = Self::get_payload_v5(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v5.record(start.elapsed());
        res
    }

    /// Fetches all the blocks for the provided range starting at `start`, containing `count`
    /// blocks and returns the mapped payload bodies.
    pub async fn get_payload_bodies_by_range_with<F, R>(
        &self,
        start: BlockNumber,
        count: u64,
        f: F,
    ) -> EngineApiResult<Vec<Option<R>>>
    where
        F: Fn(Provider::Block) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let inner = self.inner.clone();

        self.inner.task_spawner.spawn_blocking_task(Box::pin(async move {
            if count > MAX_PAYLOAD_BODIES_LIMIT {
                tx.send(Err(EngineApiError::PayloadRequestTooLarge { len: count })).ok();
                return;
            }

            if start == 0 || count == 0 {
                tx.send(Err(EngineApiError::InvalidBodiesRange { start, count })).ok();
                return;
            }

            let mut result = Vec::with_capacity(count as usize);

            // -1 so range is inclusive
            let mut end = start.saturating_add(count - 1);

            // > Client software MUST NOT return trailing null values if the request extends past the current latest known block.
            // truncate the end if it's greater than the last block
            if let Ok(best_block) = inner.provider.best_block_number()
                && end > best_block {
                    end = best_block;
                }

            // Check if the requested range starts before the earliest available block due to pruning/expiry
            let earliest_block = inner.provider.earliest_block_number().unwrap_or(0);
            for num in start..=end {
                if num < earliest_block {
                    result.push(None);
                    continue;
                }
                let block_result = inner.provider.block(BlockHashOrNumber::Number(num));
                match block_result {
                    Ok(block) => {
                        result.push(block.map(&f));
                    }
                    Err(err) => {
                        tx.send(Err(EngineApiError::Internal(Box::new(err)))).ok();
                        return;
                    }
                };
            }
            tx.send(Ok(result)).ok();
        }));

        rx.await.map_err(|err| EngineApiError::Internal(Box::new(err)))?
    }

    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// WARNING: This method is associated with the `BeaconBlocksByRange` message in the consensus
    /// layer p2p specification, meaning the input should be treated as untrusted or potentially
    /// adversarial.
    ///
    /// Implementers should take care when acting on the input to this method, specifically
    /// ensuring that the range is limited properly, and that the range boundaries are computed
    /// correctly and without panics.
    pub async fn get_payload_bodies_by_range_v1(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        self.get_payload_bodies_by_range_with(start, count, |block| ExecutionPayloadBodyV1 {
            transactions: block.body().encoded_2718_transactions(),
            withdrawals: block.body().withdrawals().cloned().map(Withdrawals::into_inner),
        })
        .await
    }

    /// Metrics version of `get_payload_bodies_by_range_v1`
    pub async fn get_payload_bodies_by_range_v1_metered(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        let start_time = Instant::now();
        let res = Self::get_payload_bodies_by_range_v1(self, start, count).await;
        self.inner.metrics.latency.get_payload_bodies_by_range_v1.record(start_time.elapsed());
        res
    }

    /// Returns the execution payload bodies by the range (V2).
    ///
    /// V2 includes the `block_access_list` field for EIP-7928 BAL support.
    pub async fn get_payload_bodies_by_range_v2(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV2> {
        self.get_payload_bodies_by_range_with(start, count, |block| ExecutionPayloadBodyV2 {
            transactions: block.body().encoded_2718_transactions(),
            withdrawals: block.body().withdrawals().cloned().map(Withdrawals::into_inner),
            block_access_list: None,
        })
        .await
    }

    /// Metrics version of `get_payload_bodies_by_range_v2`
    pub async fn get_payload_bodies_by_range_v2_metered(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV2> {
        let start_time = Instant::now();
        let res = Self::get_payload_bodies_by_range_v2(self, start, count).await;
        self.inner.metrics.latency.get_payload_bodies_by_range_v2.record(start_time.elapsed());
        res
    }

    /// Called to retrieve execution payload bodies by hashes.
    pub async fn get_payload_bodies_by_hash_with<F, R>(
        &self,
        hashes: Vec<BlockHash>,
        f: F,
    ) -> EngineApiResult<Vec<Option<R>>>
    where
        F: Fn(Provider::Block) -> R + Send + 'static,
        R: Send + 'static,
    {
        let len = hashes.len() as u64;
        if len > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(EngineApiError::PayloadRequestTooLarge { len });
        }

        let (tx, rx) = oneshot::channel();
        let inner = self.inner.clone();

        self.inner.task_spawner.spawn_blocking_task(Box::pin(async move {
            let mut result = Vec::with_capacity(hashes.len());
            for hash in hashes {
                let block_result = inner.provider.block(BlockHashOrNumber::Hash(hash));
                match block_result {
                    Ok(block) => {
                        result.push(block.map(&f));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(EngineApiError::Internal(Box::new(err))));
                        return;
                    }
                }
            }
            tx.send(Ok(result)).ok();
        }));

        rx.await.map_err(|err| EngineApiError::Internal(Box::new(err)))?
    }

    /// Called to retrieve execution payload bodies by hashes.
    pub async fn get_payload_bodies_by_hash_v1(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        self.get_payload_bodies_by_hash_with(hashes, |block| ExecutionPayloadBodyV1 {
            transactions: block.body().encoded_2718_transactions(),
            withdrawals: block.body().withdrawals().cloned().map(Withdrawals::into_inner),
        })
        .await
    }

    /// Metrics version of `get_payload_bodies_by_hash_v1`
    pub async fn get_payload_bodies_by_hash_v1_metered(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        let start = Instant::now();
        let res = Self::get_payload_bodies_by_hash_v1(self, hashes).await;
        self.inner.metrics.latency.get_payload_bodies_by_hash_v1.record(start.elapsed());
        res
    }

    /// Called to retrieve execution payload bodies by hashes (V2).
    ///
    /// V2 includes the `block_access_list` field for EIP-7928 BAL support.
    pub async fn get_payload_bodies_by_hash_v2(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV2> {
        self.get_payload_bodies_by_hash_with(hashes, |block| ExecutionPayloadBodyV2 {
            transactions: block.body().encoded_2718_transactions(),
            withdrawals: block.body().withdrawals().cloned().map(Withdrawals::into_inner),
            block_access_list: None,
        })
        .await
    }

    /// Metrics version of `get_payload_bodies_by_hash_v2`
    pub async fn get_payload_bodies_by_hash_v2_metered(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV2> {
        let start = Instant::now();
        let res = Self::get_payload_bodies_by_hash_v2(self, hashes).await;
        self.inner.metrics.latency.get_payload_bodies_by_hash_v2.record(start.elapsed());
        res
    }

    /// Validates the `engine_forkchoiceUpdated` payload attributes and executes the forkchoice
    /// update.
    ///
    /// The payload attributes will be validated according to the engine API rules for the given
    /// message version:
    /// * If the version is [`EngineApiMessageVersion::V1`], then the payload attributes will be
    ///   validated according to the Paris rules.
    /// * If the version is [`EngineApiMessageVersion::V2`], then the payload attributes will be
    ///   validated according to the Shanghai rules, as well as the validity changes from cancun:
    ///   <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/cancun.md#update-the-methods-of-previous-forks>
    ///
    /// * If the version above [`EngineApiMessageVersion::V3`], then the payload attributes will be
    ///   validated according to the Cancun rules.
    async fn validate_and_execute_forkchoice(
        &self,
        version: EngineApiMessageVersion,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attrs {
            let attr_validation_res =
                self.inner.validator.ensure_well_formed_attributes(version, attrs);

            // From the engine API spec:
            //
            // Client software MUST ensure that payloadAttributes.timestamp is greater than
            // timestamp of a block referenced by forkchoiceState.headBlockHash. If this condition
            // isn't held client software MUST respond with -38003: Invalid payload attributes and
            // MUST NOT begin a payload build process. In such an event, the forkchoiceState
            // update MUST NOT be rolled back.
            //
            // NOTE: This will also apply to the validation result for the cancun or
            // shanghai-specific fields provided in the payload attributes.
            //
            // To do this, we set the payload attrs to `None` if attribute validation failed, but
            // we still apply the forkchoice update.
            if let Err(err) = attr_validation_res {
                let fcu_res =
                    self.inner.beacon_consensus.fork_choice_updated(state, None, version).await?;
                // TODO: decide if we want this branch - the FCU INVALID response might be more
                // useful than the payload attributes INVALID response
                if fcu_res.is_invalid() {
                    return Ok(fcu_res)
                }
                return Err(err.into())
            }
        }

        Ok(self.inner.beacon_consensus.fork_choice_updated(state, payload_attrs, version).await?)
    }

    /// Returns reference to supported capabilities.
    pub fn capabilities(&self) -> &EngineCapabilities {
        &self.inner.capabilities
    }

    fn get_blobs_v1(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Vec<Option<BlobAndProofV1>>> {
        // Only allow this method before Osaka fork
        let current_timestamp =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        if self.inner.chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
            return Err(EngineApiError::EngineObjectValidationError(
                reth_payload_primitives::EngineObjectValidationError::UnsupportedFork,
            ));
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        self.inner
            .tx_pool
            .get_blobs_for_versioned_hashes_v1(&versioned_hashes)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }

    /// Metered version of `get_blobs_v1`.
    pub fn get_blobs_v1_metered(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Vec<Option<BlobAndProofV1>>> {
        let hashes_len = versioned_hashes.len();
        let start = Instant::now();
        let res = Self::get_blobs_v1(self, versioned_hashes);
        self.inner.metrics.latency.get_blobs_v1.record(start.elapsed());

        if let Ok(blobs) = &res {
            let blobs_found = blobs.iter().flatten().count();
            let blobs_missed = hashes_len - blobs_found;

            self.inner.metrics.blob_metrics.blob_count.increment(blobs_found as u64);
            self.inner.metrics.blob_metrics.blob_misses.increment(blobs_missed as u64);
        }

        res
    }

    fn get_blobs_v2(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Option<Vec<BlobAndProofV2>>> {
        // Check if Osaka fork is active
        let current_timestamp =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        if !self.inner.chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
            return Err(EngineApiError::EngineObjectValidationError(
                reth_payload_primitives::EngineObjectValidationError::UnsupportedFork,
            ));
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        self.inner
            .tx_pool
            .get_blobs_for_versioned_hashes_v2(&versioned_hashes)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }

    fn get_blobs_v3(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Option<Vec<Option<BlobAndProofV2>>>> {
        // Check if Osaka fork is active
        let current_timestamp =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        if !self.inner.chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
            return Err(EngineApiError::EngineObjectValidationError(
                reth_payload_primitives::EngineObjectValidationError::UnsupportedFork,
            ));
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        // Spec requires returning `null` if syncing.
        if (*self.inner.is_syncing)() {
            return Ok(None)
        }

        self.inner
            .tx_pool
            .get_blobs_for_versioned_hashes_v3(&versioned_hashes)
            .map(Some)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }

    /// Metered version of `get_blobs_v2`.
    pub fn get_blobs_v2_metered(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Option<Vec<BlobAndProofV2>>> {
        let hashes_len = versioned_hashes.len();
        let start = Instant::now();
        let res = Self::get_blobs_v2(self, versioned_hashes);
        self.inner.metrics.latency.get_blobs_v2.record(start.elapsed());

        if let Ok(blobs) = &res {
            let blobs_found = blobs.iter().flatten().count();

            self.inner
                .metrics
                .blob_metrics
                .get_blobs_requests_blobs_total
                .increment(hashes_len as u64);
            self.inner
                .metrics
                .blob_metrics
                .get_blobs_requests_blobs_in_blobpool_total
                .increment(blobs_found as u64);

            if blobs_found == hashes_len {
                self.inner.metrics.blob_metrics.get_blobs_requests_success_total.increment(1);
            } else {
                self.inner.metrics.blob_metrics.get_blobs_requests_failure_total.increment(1);
            }
        } else {
            self.inner.metrics.blob_metrics.get_blobs_requests_failure_total.increment(1);
        }

        res
    }

    /// Metered version of `get_blobs_v3`.
    pub fn get_blobs_v3_metered(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Option<Vec<Option<BlobAndProofV2>>>> {
        let hashes_len = versioned_hashes.len();
        let start = Instant::now();
        let res = Self::get_blobs_v3(self, versioned_hashes);
        self.inner.metrics.latency.get_blobs_v3.record(start.elapsed());

        if let Ok(Some(blobs)) = &res {
            let blobs_found = blobs.iter().flatten().count();
            let blobs_missed = hashes_len - blobs_found;

            self.inner.metrics.blob_metrics.blob_count.increment(blobs_found as u64);
            self.inner.metrics.blob_metrics.blob_misses.increment(blobs_missed as u64);
        }

        res
    }

    /// Retrieves BALs for the given block hashes from the cache.
    ///
    /// Returns the RLP-encoded BALs for blocks found in the cache or BAL store.
    /// Missing blocks are returned as empty bytes.
    pub fn get_bals_by_hash(&self, block_hashes: Vec<BlockHash>) -> Vec<alloy_primitives::Bytes> {
        self.inner
            .bal_provider
            .get_by_hashes(&block_hashes, &self.inner.metrics.bal_metrics)
            .into_iter()
            .map(|opt| opt.unwrap_or_default())
            .collect()
    }

    /// Retrieves BALs for a range of blocks from the cache or BAL store.
    ///
    /// Returns the RLP-encoded BALs for blocks in the range `[start, start + count)`.
    pub fn get_bals_by_range(&self, start: u64, count: u64) -> Vec<alloy_primitives::Bytes> {
        self.inner.bal_provider.get_by_range(start, count, &self.inner.metrics.bal_metrics)
    }
}

// This is the concrete ethereum engine API implementation.
#[async_trait]
impl<Provider, EngineT, Pool, Validator, ChainSpec> EngineApiServer<EngineT>
    for EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    EngineT: EngineTypes<ExecutionData = ExecutionData>,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EngineT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    /// Handler for `engine_newPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV1");
        let payload =
            ExecutionData { payload: payload.into(), sidecar: ExecutionPayloadSidecar::none() };
        Ok(self.new_payload_v1_metered(payload).await?)
    }

    /// Handler for `engine_newPayloadV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV2");
        let payload = ExecutionData {
            payload: payload.into_payload(),
            sidecar: ExecutionPayloadSidecar::none(),
        };

        Ok(self.new_payload_v2_metered(payload).await?)
    }

    /// Handler for `engine_newPayloadV3`
    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV3");
        let payload = ExecutionData {
            payload: payload.into(),
            sidecar: ExecutionPayloadSidecar::v3(CancunPayloadFields {
                versioned_hashes,
                parent_beacon_block_root,
            }),
        };

        Ok(self.new_payload_v3_metered(payload).await?)
    }

    /// Handler for `engine_newPayloadV4`
    /// See also <https://github.com/ethereum/execution-apis/blob/03911ffc053b8b806123f1fc237184b0092a485a/src/engine/prague.md#engine_newpayloadv4>
    async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        requests: RequestsOrHash,
    ) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV4");

        // Accept requests as a hash only if it is explicitly allowed
        if requests.is_hash() && !self.inner.accept_execution_requests_hash {
            return Err(EngineApiError::UnexpectedRequestsHash.into());
        }

        let payload = ExecutionData {
            payload: payload.into(),
            sidecar: ExecutionPayloadSidecar::v4(
                CancunPayloadFields { versioned_hashes, parent_beacon_block_root },
                PraguePayloadFields { requests },
            ),
        };

        Ok(self.new_payload_v4_metered(payload).await?)
    }

    /// Handler for `engine_newPayloadV5`
    ///
    /// Post Amsterdam payload handler. Currently returns unsupported fork error.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/amsterdam.md#engine_newpayloadv5>
    async fn new_payload_v5(
        &self,
        _payload: ExecutionPayloadV4,
        _versioned_hashes: Vec<B256>,
        _parent_beacon_block_root: B256,
        _execution_requests: RequestsOrHash,
    ) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV5");
        Err(EngineApiError::EngineObjectValidationError(
            reth_payload_primitives::EngineObjectValidationError::UnsupportedFork,
        ))?
    }

    /// Handler for `engine_forkchoiceUpdatedV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_forkchoiceupdatedv1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV1");
        Ok(self.fork_choice_updated_v1_metered(fork_choice_state, payload_attributes).await?)
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV2");
        Ok(self.fork_choice_updated_v2_metered(fork_choice_state, payload_attributes).await?)
    }

    /// Handler for `engine_forkchoiceUpdatedV3`
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_forkchoiceupdatedv3>
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV3");
        Ok(self.fork_choice_updated_v3_metered(fork_choice_state, payload_attributes).await?)
    }

    /// Handler for `engine_getPayloadV1`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV1");
        Ok(self.get_payload_v1_metered(payload_id).await?)
    }

    /// Handler for `engine_getPayloadV2`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_getpayloadv2>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV2> {
        debug!(target: "rpc::engine", id = %payload_id, "Serving engine_getPayloadV2");
        Ok(self.get_payload_v2_metered(payload_id).await?)
    }

    /// Handler for `engine_getPayloadV3`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_getpayloadv3>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV3> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV3");
        Ok(self.get_payload_v3_metered(payload_id).await?)
    }

    /// Handler for `engine_getPayloadV4`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_getpayloadv4>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV4> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV4");
        Ok(self.get_payload_v4_metered(payload_id).await?)
    }

    /// Handler for `engine_getPayloadV5`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/15399c2e2f16a5f800bf3f285640357e2c245ad9/src/engine/osaka.md#engine_getpayloadv5>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v5(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV5> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV5");
        Ok(self.get_payload_v5_metered(payload_id).await?)
    }

    /// Handler for `engine_getPayloadV6`
    ///
    /// Post Amsterdam payload handler. Currently returns unsupported fork error.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/amsterdam.md#engine_getpayloadv6>
    async fn get_payload_v6(
        &self,
        _payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV6> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV6");
        Err(EngineApiError::EngineObjectValidationError(
            reth_payload_primitives::EngineObjectValidationError::UnsupportedFork,
        ))?
    }

    /// Handler for `engine_getPayloadBodiesByHashV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByHashV1");
        Ok(self.get_payload_bodies_by_hash_v1_metered(block_hashes).await?)
    }

    /// Handler for `engine_getPayloadBodiesByHashV2`
    ///
    /// V2 includes the `block_access_list` field for EIP-7928 BAL support.
    ///
    /// See also <https://eips.ethereum.org/EIPS/eip-7928>
    async fn get_payload_bodies_by_hash_v2(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV2> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByHashV2");
        Ok(self.get_payload_bodies_by_hash_v2_metered(block_hashes).await?)
    }

    /// Handler for `engine_getPayloadBodiesByRangeV1`
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyrangev1>
    ///
    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// WARNING: This method is associated with the `BeaconBlocksByRange` message in the consensus
    /// layer p2p specification, meaning the input should be treated as untrusted or potentially
    /// adversarial.
    ///
    /// Implementers should take care when acting on the input to this method, specifically
    /// ensuring that the range is limited properly, and that the range boundaries are computed
    /// correctly and without panics.
    ///
    /// Note: If a block is pre shanghai, `withdrawals` field will be `null`.
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByRangeV1");
        Ok(self.get_payload_bodies_by_range_v1_metered(start.to(), count.to()).await?)
    }

    /// Handler for `engine_getPayloadBodiesByRangeV2`
    ///
    /// V2 includes the `block_access_list` field for EIP-7928 BAL support.
    ///
    /// See also <https://eips.ethereum.org/EIPS/eip-7928>
    async fn get_payload_bodies_by_range_v2(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV2> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByRangeV2");
        Ok(self.get_payload_bodies_by_range_v2_metered(start.to(), count.to()).await?)
    }

    /// Handler for `engine_getClientVersionV1`
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/03911ffc053b8b806123f1fc237184b0092a485a/src/engine/identification.md>
    async fn get_client_version_v1(
        &self,
        client: ClientVersionV1,
    ) -> RpcResult<Vec<ClientVersionV1>> {
        trace!(target: "rpc::engine", "Serving engine_getClientVersionV1");
        Ok(Self::get_client_version_v1(self, client)?)
    }

    /// Handler for `engine_exchangeCapabilitiesV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        trace!(target: "rpc::engine", "Serving engine_exchangeCapabilities");

        let el_caps = self.capabilities();
        el_caps.log_capability_mismatches(&capabilities);

        Ok(el_caps.list())
    }

    async fn get_blobs_v1(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV1>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV1");
        Ok(self.get_blobs_v1_metered(versioned_hashes)?)
    }

    async fn get_blobs_v2(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Option<Vec<BlobAndProofV2>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV2");
        Ok(self.get_blobs_v2_metered(versioned_hashes)?)
    }

    async fn get_blobs_v3(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Option<Vec<Option<BlobAndProofV2>>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV3");
        Ok(self.get_blobs_v3_metered(versioned_hashes)?)
    }
}

impl<Provider, EngineT, Pool, Validator, ChainSpec> IntoEngineApiRpcModule
    for EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    EngineT: EngineTypes,
    Self: EngineApiServer<EngineT>,
{
    fn into_rpc_module(self) -> RpcModule<()> {
        self.into_rpc().remove_context()
    }
}

impl<Provider, PayloadT, Pool, Validator, ChainSpec> std::fmt::Debug
    for EngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>
where
    PayloadT: PayloadTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").finish_non_exhaustive()
    }
}

impl<Provider, PayloadT, Pool, Validator, ChainSpec> Clone
    for EngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>
where
    PayloadT: PayloadTypes,
{
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

/// The container type for the engine API internals.
struct EngineApiInner<Provider, PayloadT: PayloadTypes, Pool, Validator, ChainSpec> {
    /// The provider to interact with the chain.
    provider: Provider,
    /// Consensus configuration
    chain_spec: Arc<ChainSpec>,
    /// The channel to send messages to the beacon consensus engine.
    beacon_consensus: ConsensusEngineHandle<PayloadT>,
    /// The type that can communicate with the payload service to retrieve payloads.
    payload_store: PayloadStore<PayloadT>,
    /// For spawning and executing async tasks
    task_spawner: Box<dyn TaskSpawner>,
    /// The latency and response type metrics for engine api calls
    metrics: EngineApiMetrics,
    /// Identification of the execution client used by the consensus client
    client: ClientVersionV1,
    /// The list of all supported Engine capabilities available over the engine endpoint.
    capabilities: EngineCapabilities,
    /// Transaction pool.
    tx_pool: Pool,
    /// Engine validator.
    validator: Validator,
    accept_execution_requests_hash: bool,
    /// Returns `true` if the node is currently syncing.
    is_syncing: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Block Access List (BAL) provider with cache-first fallback semantics.
    bal_provider: BalProvider,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rpc_types_engine::{ClientCode, ClientVersionV1};
    use assert_matches::assert_matches;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
    use reth_engine_primitives::BeaconEngineMessage;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_ethereum_primitives::Block;
    use reth_network_api::{
        noop::NoopNetwork, EthProtocolInfo, NetworkError, NetworkInfo, NetworkStatus,
    };
    use reth_node_ethereum::EthereumEngineValidator;
    use reth_payload_builder::test_utils::spawn_test_payload_service;
    use reth_provider::test_utils::MockEthProvider;
    use reth_tasks::TokioTaskExecutor;
    use reth_transaction_pool::noop::NoopTransactionPool;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn setup_engine_api() -> (
        EngineApiTestHandle,
        EngineApi<
            Arc<MockEthProvider>,
            EthEngineTypes,
            NoopTransactionPool,
            EthereumEngineValidator,
            ChainSpec,
        >,
    ) {
        let client = ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".to_string(),
            version: "v0.2.0-beta.5".to_string(),
            commit: "defa64b2".to_string(),
        };

        let chain_spec: Arc<ChainSpec> = MAINNET.clone();
        let provider = Arc::new(MockEthProvider::default());
        let payload_store = spawn_test_payload_service();
        let (to_engine, engine_rx) = unbounded_channel();
        let task_executor = Box::<TokioTaskExecutor>::default();
        let api = EngineApi::new(
            provider.clone(),
            chain_spec.clone(),
            ConsensusEngineHandle::new(to_engine),
            payload_store.into(),
            NoopTransactionPool::default(),
            task_executor,
            client,
            EngineCapabilities::default(),
            EthereumEngineValidator::new(chain_spec.clone()),
            false,
            NoopNetwork::default(),
            Arc::new(crate::bal_store::NoopBalStore),
        );
        let handle = EngineApiTestHandle { chain_spec, provider, from_api: engine_rx };
        (handle, api)
    }

    #[tokio::test]
    async fn engine_client_version_v1() {
        let client = ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".to_string(),
            version: "v0.2.0-beta.5".to_string(),
            commit: "defa64b2".to_string(),
        };
        let (_, api) = setup_engine_api();
        let res = api.get_client_version_v1(client.clone());
        assert_eq!(res.unwrap(), vec![client]);
    }

    struct EngineApiTestHandle {
        #[allow(dead_code)]
        chain_spec: Arc<ChainSpec>,
        provider: Arc<MockEthProvider>,
        from_api: UnboundedReceiver<BeaconEngineMessage<EthEngineTypes>>,
    }

    #[tokio::test]
    async fn forwards_responses_to_consensus_engine() {
        let (mut handle, api) = setup_engine_api();

        tokio::spawn(async move {
            let payload_v1 = ExecutionPayloadV1::from_block_slow(&Block::default());
            let execution_data = ExecutionData {
                payload: payload_v1.into(),
                sidecar: ExecutionPayloadSidecar::none(),
            };

            api.new_payload_v1(execution_data).await.unwrap();
        });
        assert_matches!(handle.from_api.recv().await, Some(BeaconEngineMessage::NewPayload { .. }));
    }

    #[derive(Clone)]
    struct TestNetworkInfo {
        syncing: bool,
    }

    impl NetworkInfo for TestNetworkInfo {
        fn local_addr(&self) -> std::net::SocketAddr {
            (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
        }

        async fn network_status(&self) -> Result<NetworkStatus, NetworkError> {
            #[allow(deprecated)]
            Ok(NetworkStatus {
                client_version: "test".to_string(),
                protocol_version: 5,
                eth_protocol_info: EthProtocolInfo {
                    network: 1,
                    difficulty: None,
                    genesis: Default::default(),
                    config: Default::default(),
                    head: Default::default(),
                },
                capabilities: vec![],
            })
        }

        fn chain_id(&self) -> u64 {
            1
        }

        fn is_syncing(&self) -> bool {
            self.syncing
        }

        fn is_initially_syncing(&self) -> bool {
            self.syncing
        }
    }

    #[tokio::test]
    async fn get_blobs_v3_returns_null_when_syncing() {
        let chain_spec: Arc<ChainSpec> =
            Arc::new(ChainSpecBuilder::mainnet().osaka_activated().build());
        let provider = Arc::new(MockEthProvider::default());
        let payload_store = spawn_test_payload_service::<EthEngineTypes>();
        let (to_engine, _engine_rx) = unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();

        let api = EngineApi::new(
            provider,
            chain_spec.clone(),
            ConsensusEngineHandle::new(to_engine),
            payload_store.into(),
            NoopTransactionPool::default(),
            Box::<TokioTaskExecutor>::default(),
            ClientVersionV1 {
                code: ClientCode::RH,
                name: "Reth".to_string(),
                version: "v0.0.0-test".to_string(),
                commit: "test".to_string(),
            },
            EngineCapabilities::default(),
            EthereumEngineValidator::new(chain_spec),
            false,
            TestNetworkInfo { syncing: true },
            Arc::new(crate::bal_store::NoopBalStore),
        );

        let res = api.get_blobs_v3_metered(vec![B256::ZERO]);
        assert_matches!(res, Ok(None));
    }

    // tests covering `engine_getPayloadBodiesByRange` and `engine_getPayloadBodiesByHash`
    mod get_payload_bodies {
        use super::*;
        use alloy_rpc_types_engine::ExecutionPayloadBodyV1;
        use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};

        #[tokio::test]
        async fn invalid_params() {
            let (_, api) = setup_engine_api();

            let by_range_tests = [
                // (start, count)
                (0, 0),
                (0, 1),
                (1, 0),
            ];

            // test [EngineApiMessage::GetPayloadBodiesByRange]
            for (start, count) in by_range_tests {
                let res = api.get_payload_bodies_by_range_v1(start, count).await;
                assert_matches!(res, Err(EngineApiError::InvalidBodiesRange { .. }));
            }
        }

        #[tokio::test]
        async fn request_too_large() {
            let (_, api) = setup_engine_api();

            let request_count = MAX_PAYLOAD_BODIES_LIMIT + 1;
            let res = api.get_payload_bodies_by_range_v1(0, request_count).await;
            assert_matches!(res, Err(EngineApiError::PayloadRequestTooLarge { .. }));
        }

        #[tokio::test]
        async fn returns_payload_bodies() {
            let mut rng = generators::rng();
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 10);
            let blocks = random_block_range(
                &mut rng,
                start..=start + count - 1,
                BlockRangeParams { tx_count: 0..2, ..Default::default() },
            );
            handle
                .provider
                .extend_blocks(blocks.iter().cloned().map(|b| (b.hash(), b.into_block())));

            let expected = blocks
                .iter()
                .cloned()
                .map(|b| Some(ExecutionPayloadBodyV1::from_block(b.into_block())))
                .collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range_v1(start, count).await.unwrap();
            assert_eq!(res, expected);
        }

        #[tokio::test]
        async fn returns_payload_bodies_with_gaps() {
            let mut rng = generators::rng();
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 100);
            let blocks = random_block_range(
                &mut rng,
                start..=start + count - 1,
                BlockRangeParams { tx_count: 0..2, ..Default::default() },
            );

            // Insert only blocks in ranges 1-25 and 50-75
            let first_missing_range = 26..=50;
            let second_missing_range = 76..=100;
            handle.provider.extend_blocks(
                blocks
                    .iter()
                    .filter(|b| {
                        !first_missing_range.contains(&b.number) &&
                            !second_missing_range.contains(&b.number)
                    })
                    .map(|b| (b.hash(), b.clone().into_block())),
            );

            let expected = blocks
                .iter()
                // filter anything after the second missing range to ensure we don't expect trailing
                // `None`s
                .filter(|b| !second_missing_range.contains(&b.number))
                .cloned()
                .map(|b| {
                    if first_missing_range.contains(&b.number) {
                        None
                    } else {
                        Some(ExecutionPayloadBodyV1::from_block(b.into_block()))
                    }
                })
                .collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range_v1(start, count).await.unwrap();
            assert_eq!(res, expected);

            let expected = blocks
                .iter()
                .cloned()
                // ensure we still return trailing `None`s here because by-hash will not be aware
                // of the missing block's number, and cannot compare it to the current best block
                .map(|b| {
                    if first_missing_range.contains(&b.number) ||
                        second_missing_range.contains(&b.number)
                    {
                        None
                    } else {
                        Some(ExecutionPayloadBodyV1::from_block(b.into_block()))
                    }
                })
                .collect::<Vec<_>>();

            let hashes = blocks.iter().map(|b| b.hash()).collect();
            let res = api.get_payload_bodies_by_hash_v1(hashes).await.unwrap();
            assert_eq!(res, expected);
        }
    }

    mod bal_queries {
        use super::*;
        use alloy_rpc_types_engine::ClientCode;
        use parking_lot::{Mutex, RwLock};
        use std::{
            collections::{BTreeMap, HashMap},
            future::Future,
            sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            },
        };

        #[derive(Debug, Clone, Default)]
        struct TestBalStore {
            by_hash: Arc<RwLock<HashMap<BlockHash, Bytes>>>,
            by_number: Arc<RwLock<BTreeMap<BlockNumber, BlockHash>>>,
            hash_queries: Arc<AtomicUsize>,
            range_queries: Arc<Mutex<Vec<(BlockNumber, u64)>>>,
        }

        impl TestBalStore {
            fn insert_raw(&self, block_hash: BlockHash, block_number: BlockNumber, bal: Bytes) {
                self.by_hash.write().insert(block_hash, bal);
                self.by_number.write().insert(block_number, block_hash);
            }

            fn hash_query_count(&self) -> usize {
                self.hash_queries.load(Ordering::Relaxed)
            }

            fn range_queries(&self) -> Vec<(BlockNumber, u64)> {
                self.range_queries.lock().clone()
            }
        }

        impl BalStore for TestBalStore {
            fn insert(
                &self,
                block_hash: BlockHash,
                block_number: BlockNumber,
                bal: Bytes,
            ) -> Result<(), BalStoreError> {
                self.insert_raw(block_hash, block_number, bal);
                Ok(())
            }

            fn get_by_hashes(
                &self,
                block_hashes: &[BlockHash],
            ) -> Result<Vec<Option<Bytes>>, BalStoreError> {
                self.hash_queries.fetch_add(1, Ordering::Relaxed);
                let by_hash = self.by_hash.read();
                Ok(block_hashes.iter().map(|hash| by_hash.get(hash).cloned()).collect())
            }

            fn get_by_range(
                &self,
                start: BlockNumber,
                count: u64,
            ) -> Result<Vec<Bytes>, BalStoreError> {
                self.range_queries.lock().push((start, count));
                let by_hash = self.by_hash.read();
                let by_number = self.by_number.read();
                let mut result = Vec::new();

                for block_number in start..start.saturating_add(count) {
                    let Some(hash) = by_number.get(&block_number) else {
                        break;
                    };
                    let Some(bal) = by_hash.get(hash) else {
                        break;
                    };
                    result.push(bal.clone());
                }
                Ok(result)
            }
        }

        fn setup_engine_api_with_store(
            bal_store: Arc<dyn BalStore>,
        ) -> EngineApi<
            Arc<MockEthProvider>,
            EthEngineTypes,
            NoopTransactionPool,
            EthereumEngineValidator,
            ChainSpec,
        > {
            let chain_spec: Arc<ChainSpec> = MAINNET.clone();
            let provider = Arc::new(MockEthProvider::default());
            let payload_store = spawn_test_payload_service();
            let (to_engine, _) = unbounded_channel();

            EngineApi::with_bal_store(
                provider,
                chain_spec.clone(),
                ConsensusEngineHandle::new(to_engine),
                payload_store.into(),
                NoopTransactionPool::default(),
                Box::<TokioTaskExecutor>::default(),
                ClientVersionV1 {
                    code: ClientCode::RH,
                    name: "Reth".to_string(),
                    version: "v0.0.0-test".to_string(),
                    commit: "test".to_string(),
                },
                EngineCapabilities::default(),
                EthereumEngineValidator::new(chain_spec),
                false,
                NoopNetwork::default(),
                bal_store,
            )
        }

        fn run_with_tokio_runtime(test: impl Future<Output = ()>) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime for test");
            runtime.block_on(test);
        }

        #[test]
        fn by_hash_uses_partial_store_fallback() {
            run_with_tokio_runtime(async {
                let store = TestBalStore::default();
                let hash1 = B256::random();
                let hash2 = B256::random();
                let hash3 = B256::random();
                let bal1 = Bytes::from_static(b"bal1");
                let bal2 = Bytes::from_static(b"bal2");

                store.insert_raw(hash2, 2, bal2.clone());
                let api = setup_engine_api_with_store(Arc::new(store.clone()));
                api.bal_cache().insert(hash1, 1, bal1.clone());

                let results = api.get_bals_by_hash(vec![hash1, hash2, hash3]);
                assert_eq!(results, vec![bal1, bal2, Bytes::new()]);
                assert_eq!(store.hash_query_count(), 1);
            });
        }

        #[test]
        fn by_range_fallback_queries_only_missing_suffix() {
            run_with_tokio_runtime(async {
                let store = TestBalStore::default();
                let hash1 = B256::random();
                let hash2 = B256::random();
                let hash3 = B256::random();
                let hash4 = B256::random();
                let hash5 = B256::random();

                let bal1 = Bytes::from_static(b"bal1");
                let bal2 = Bytes::from_static(b"bal2");
                let bal3 = Bytes::from_static(b"bal3");
                let bal4 = Bytes::from_static(b"bal4");
                let bal5 = Bytes::from_static(b"bal5");

                store.insert_raw(hash3, 3, bal3.clone());
                store.insert_raw(hash4, 4, bal4.clone());
                store.insert_raw(hash5, 5, bal5.clone());

                let api = setup_engine_api_with_store(Arc::new(store.clone()));
                api.bal_cache().insert(hash1, 1, bal1.clone());
                api.bal_cache().insert(hash2, 2, bal2.clone());

                let results = api.get_bals_by_range(1, 5);
                assert_eq!(results, vec![bal1, bal2, bal3, bal4, bal5]);
                assert_eq!(store.range_queries(), vec![(3, 3)]);
            });
        }
    }
}
