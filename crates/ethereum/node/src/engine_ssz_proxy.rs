//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v2`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{
    eip1898::BlockHashOrNumber,
    eip2718::Decodable2718,
    eip4895::Withdrawals,
    eip7685::{Requests, RequestsOrHash},
};
use alloy_primitives::{BlockHash, BlockNumber, Bytes, Sealable, B128, B256};
use alloy_rpc_types_engine::{
    ssz_engine_types::{
        BodiesByHashRequest, BodiesResponse, BodiesResponseAmsterdam, BodiesResponseCancun,
        BodiesResponseOsaka, BodiesResponseParis, BodiesResponsePrague, BodiesResponseShanghai,
        BodyEntry, ExecutionPayloadBodyAmsterdam, ExecutionPayloadBodyCancun,
        ExecutionPayloadBodyOsaka, ExecutionPayloadBodyParis, ExecutionPayloadBodyPrague,
        ExecutionPayloadBodyShanghai,
    },
    CancunPayloadFields, ClientVersionV1, ExecutionData, ExecutionPayload, ExecutionPayloadBodyV1,
    ExecutionPayloadBodyV2, ExecutionPayloadSidecar, ExecutionPayloadV1, ExecutionPayloadV2,
    ExecutionPayloadV3, ExecutionPayloadV4, ForkchoiceState, PayloadAttributes,
    PraguePayloadFields,
};
use http_body_util::BodyExt;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_core::version::{version_metadata, CLIENT_CODE};
use reth_payload_primitives::EngineObjectValidationError;
use reth_primitives_traits::{Block, BlockBody};
use reth_provider::{BalProvider, BlockNumReader, BlockReader, HeaderProvider};
use reth_transaction_pool::BlobStore;
use ssz::Decode;
use ssz_types::{typenum::U32, VariableList};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::SystemTime,
};
use tokio::sync::RwLock;
use tower::{BoxError, Layer, Service};

const OCTET_STREAM: &str = "application/octet-stream";
const APPLICATION_JSON: &str = "application/json";
const TEXT_PLAIN: &str = "text/plain";
const CONTENT_TYPE: &str = "content-type";

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
const STATUS_PAYLOAD_TOO_LARGE: u16 = 413;
const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_NOT_IMPLEMENTED: u16 = 501;
const STATUS_SERVICE_UNAVAILABLE: u16 = 503;

const MAX_PAYLOAD_BODIES_LIMIT: u64 = 32;
const MAX_BLOB_LIMIT: usize = 128;
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// Shared handle used by [`EngineSszProxyLayer`].
pub struct EngineSszProxyHandle<ChainSpec, Provider> {
    engine: Arc<RwLock<Option<ConsensusEngineHandle<EthEngineTypes>>>>,
    blob_store: Arc<RwLock<Option<Arc<dyn BlobStore>>>>,
    client_version: Arc<RwLock<Option<ClientVersionV1>>>,
    provider: Arc<RwLock<Option<Provider>>>,
    chain_spec: Arc<ChainSpec>,
}

impl<C, P> Clone for EngineSszProxyHandle<C, P> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            blob_store: self.blob_store.clone(),
            client_version: self.client_version.clone(),
            provider: self.provider.clone(),
            chain_spec: self.chain_spec.clone(),
        }
    }
}

impl<C, P> std::fmt::Debug for EngineSszProxyHandle<C, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSszProxyHandle").finish_non_exhaustive()
    }
}

impl<ChainSpec, Provider> EngineSszProxyHandle<ChainSpec, Provider> {
    fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            engine: Default::default(),
            blob_store: Default::default(),
            client_version: Arc::new(RwLock::new(Some(default_client_version()))),
            provider: Default::default(),
            chain_spec,
        }
    }

    /// Sets the consensus engine handle used by the proxy.
    pub async fn set_engine(&self, engine: ConsensusEngineHandle<EthEngineTypes>) {
        *self.engine.write().await = Some(engine);
    }

    async fn engine(&self) -> Option<ConsensusEngineHandle<EthEngineTypes>> {
        self.engine.read().await.clone()
    }

    /// Sets the blob store used by the proxy for handling blob requests.
    pub async fn set_blob_store(&self, blob_store: impl BlobStore) {
        *self.blob_store.write().await = Some(Arc::new(blob_store));
    }

    async fn blob_store(&self) -> Option<Arc<dyn BlobStore>> {
        self.blob_store.read().await.clone()
    }

    /// Sets the client version returned by the identity endpoint.
    pub async fn set_client_version(&self, client_version: ClientVersionV1) {
        *self.client_version.write().await = Some(client_version);
    }

    async fn client_version(&self) -> Option<ClientVersionV1> {
        self.client_version.read().await.clone()
    }

    /// Sets the provider used by the proxy for handling payload body requests.
    pub async fn set_provider(&self, provider: Provider) {
        *self.provider.write().await = Some(provider);
    }
}

impl<ChainSpec, Provider> EngineSszProxyHandle<ChainSpec, Provider>
where
    Provider: Clone,
{
    async fn provider(&self) -> Option<Provider> {
        self.provider.read().await.clone()
    }
}

impl<ChainSpec, Provider> EngineSszProxyHandle<ChainSpec, Provider> {
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec.clone()
    }
}

/// A tower layer that intercepts SSZ Engine API routes under `/engine/v2`.
#[derive(Clone, Debug)]
pub struct EngineSszProxyLayer<ChainSpec, Provider> {
    handle: EngineSszProxyHandle<ChainSpec, Provider>,
}

impl<ChainSpec, Provider> EngineSszProxyLayer<ChainSpec, Provider> {
    /// Creates a new proxy layer and a handle for setting the engine after node launch.
    pub fn new(chain_spec: Arc<ChainSpec>) -> (Self, EngineSszProxyHandle<ChainSpec, Provider>) {
        let handle = EngineSszProxyHandle::new(chain_spec);
        (Self { handle: handle.clone() }, handle)
    }
}

impl<S, ChainSpec, Provider> Layer<S> for EngineSszProxyLayer<ChainSpec, Provider> {
    type Service = EngineSszProxyService<S, ChainSpec, Provider>;

    fn layer(&self, inner: S) -> Self::Service {
        EngineSszProxyService { inner, handle: self.handle.clone() }
    }
}

/// The service produced by [`EngineSszProxyLayer`].
#[derive(Clone, Debug)]
pub struct EngineSszProxyService<S, ChainSpec, Provider> {
    inner: S,
    handle: EngineSszProxyHandle<ChainSpec, Provider>,
}

impl<S, ChainSpec, Provider> Service<HttpRequest> for EngineSszProxyService<S, ChainSpec, Provider>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError> + Send + Clone,
    S::Future: Send + 'static,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
    Provider: Clone + HeaderProvider + BlockReader + BalProvider + Send + Sync + 'static,
    Provider::Block: Send,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: HttpRequest) -> Self::Future {
        if !request.uri().path().starts_with("/engine/") {
            let fut = self.inner.call(request);
            return Box::pin(fut)
        }

        let handle = self.handle.clone();
        Box::pin(async move { Ok(handle_engine_ssz_request(handle, request).await) })
    }
}

async fn handle_engine_ssz_request<ChainSpec, Provider>(
    handle: EngineSszProxyHandle<ChainSpec, Provider>,
    request: HttpRequest,
) -> HttpResponse
where
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
    Provider: Clone + HeaderProvider + BlockReader + BalProvider + Send + Sync + 'static,
    Provider::Block: Send,
{
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let Some(endpoint) = parse_engine_path(&path) else {
        return text_response(STATUS_NOT_FOUND, "unknown engine ssz endpoint")
    };

    match endpoint {
        EngineSszEndpoint::Capabilities => {
            if method != "GET" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            handle_capabilities()
        }
        EngineSszEndpoint::Identity => {
            if method != "GET" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            handle_identity(handle.client_version().await)
        }
        EngineSszEndpoint::Payloads(fork) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine) = handle.engine().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine handle unavailable")
            };
            handle_new_payload(engine, fork.payloads_version(), &body).await
        }
        EngineSszEndpoint::Forkchoice(fork) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine) = handle.engine().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine handle unavailable")
            };
            handle_forkchoice_updated(engine, fork.forkchoice_version(), &body).await
        }
        EngineSszEndpoint::BodiesByHash(fork) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Some(provider) = handle.provider().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "provider unavailable")
            };
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let hashes = match decode_payload_bodies_by_hash_request(&body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            handle_get_payload_bodies_by_hash(provider, fork, hashes).await
        }
        EngineSszEndpoint::BodiesByRange(fork) => {
            if method != "GET" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Some(provider) = handle.provider().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "provider unavailable")
            };
            let Some((start, count)) = parse_bodies_range_query(query.as_deref()) else {
                return text_response(STATUS_BAD_REQUEST, "invalid bodies range query")
            };
            handle_get_payload_bodies_by_range(provider, fork, start, count).await
        }
        EngineSszEndpoint::Blobs(version) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            handle_get_blobs(handle, version, &body).await
        }
    }
}

fn parse_engine_path(path: &str) -> Option<EngineSszEndpoint> {
    let mut segments = path.trim_start_matches('/').split('/');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("engine"), Some("v2"), Some("capabilities"), None, None, None) => {
            Some(EngineSszEndpoint::Capabilities)
        }
        (Some("engine"), Some("v2"), Some("identity"), None, None, None) => {
            Some(EngineSszEndpoint::Identity)
        }
        (Some("engine"), Some("v2"), Some(fork), Some("payloads"), None, None) => {
            Some(EngineSszEndpoint::Payloads(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("forkchoice"), None, None) => {
            Some(EngineSszEndpoint::Forkchoice(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("bodies"), Some("hash"), None) => {
            Some(EngineSszEndpoint::BodiesByHash(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("bodies"), None, None) => {
            Some(EngineSszEndpoint::BodiesByRange(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some("blobs"), version, None, None) => {
            Some(EngineSszEndpoint::Blobs(parse_method_version(version?)?))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EngineSszEndpoint {
    Capabilities,
    Identity,
    Payloads(EngineSszFork),
    Forkchoice(EngineSszFork),
    BodiesByHash(EngineSszFork),
    BodiesByRange(EngineSszFork),
    Blobs(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EngineSszFork {
    Paris,
    Shanghai,
    Cancun,
    Prague,
    Osaka,
    Amsterdam,
}

impl EngineSszFork {
    const fn payloads_version(self) -> u8 {
        match self {
            Self::Paris => 1,
            Self::Shanghai => 2,
            Self::Cancun => 3,
            Self::Prague | Self::Osaka => 4,
            Self::Amsterdam => 5,
        }
    }

    const fn forkchoice_version(self) -> u8 {
        match self {
            Self::Paris => 1,
            Self::Shanghai => 2,
            Self::Cancun | Self::Prague | Self::Osaka => 3,
            Self::Amsterdam => 4,
        }
    }
}

impl std::str::FromStr for EngineSszFork {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "paris" => Ok(Self::Paris),
            "shanghai" => Ok(Self::Shanghai),
            "cancun" => Ok(Self::Cancun),
            "prague" => Ok(Self::Prague),
            "osaka" => Ok(Self::Osaka),
            "amsterdam" => Ok(Self::Amsterdam),
            _ => Err(()),
        }
    }
}

fn parse_method_version(version: &str) -> Option<u8> {
    version.strip_prefix('v')?.parse().ok().filter(|version| (1..=4).contains(version))
}

fn default_client_version() -> ClientVersionV1 {
    ClientVersionV1 {
        code: CLIENT_CODE,
        name: version_metadata().name_client.to_string(),
        version: version_metadata().cargo_pkg_version.to_string(),
        commit: version_metadata().vergen_git_sha.to_string(),
    }
}

fn handle_capabilities() -> HttpResponse {
    json_response(serde_json::json!({
        "supported_forks": ["paris", "shanghai", "cancun", "prague", "osaka", "amsterdam"],
        "fork_scoped_endpoints": ["payloads", "forkchoice", "bodies"],
        "independently_versioned": {
            "blobs": ["v1", "v2", "v3", "v4"],
        },
        "unscoped_endpoints": ["capabilities", "identity"],
        "limits": {
            "bodies.max_count": MAX_PAYLOAD_BODIES_LIMIT,
            "blobs.max_versioned_hashes": MAX_BLOB_LIMIT,
            "payload.max_bytes": MAX_PAYLOAD_BYTES,
        },
    }))
}

fn handle_identity(client_version: Option<ClientVersionV1>) -> HttpResponse {
    json_response(client_version.into_iter().collect::<Vec<_>>())
}

fn parse_bodies_range_query(query: Option<&str>) -> Option<(u64, u64)> {
    let mut start = None;
    let mut count = None;

    for param in query?.split('&') {
        let (key, value) = param.split_once('=')?;
        match key {
            "from" => start = Some(value.parse().ok()?),
            "count" => count = Some(value.parse().ok()?),
            _ => return None,
        }
    }

    Some((start?, count?))
}

async fn handle_new_payload(
    engine: ConsensusEngineHandle<EthEngineTypes>,
    version: u8,
    body: &[u8],
) -> HttpResponse {
    let payload = match decode_new_payload_request(version, body) {
        Ok(payload) => payload,
        Err(err) => return text_response(STATUS_BAD_REQUEST, err),
    };

    match engine.new_payload(payload).await {
        Ok(status) => ssz_response(status),
        Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn handle_forkchoice_updated(
    engine: ConsensusEngineHandle<EthEngineTypes>,
    version: u8,
    body: &[u8],
) -> HttpResponse {
    let (state, attrs) = match decode_forkchoice_request(version, body) {
        Ok(request) => request,
        Err(err) => return text_response(STATUS_BAD_REQUEST, err),
    };

    match engine.fork_choice_updated(state, attrs).await {
        Ok(updated) => ssz_response(updated),
        Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn handle_get_payload_bodies_by_hash<Provider>(
    provider: Provider,
    fork: EngineSszFork,
    block_hashes: Vec<B256>,
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + BalProvider + Send + 'static,
    Provider::Block: Send,
{
    match get_payload_bodies_by_hash(provider, fork, block_hashes).await {
        Ok(PayloadBodies::Paris(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Shanghai(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Cancun(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Prague(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Osaka(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Amsterdam(bodies)) => ssz_response(bodies),
        Err(err) => payload_bodies_error_response(err),
    }
}

async fn handle_get_payload_bodies_by_range<Provider>(
    provider: Provider,
    fork: EngineSszFork,
    start: u64,
    count: u64,
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + BalProvider + Send + 'static,
    Provider::Block: Send,
{
    match get_payload_bodies_by_range(provider, fork, start, count).await {
        Ok(PayloadBodies::Paris(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Shanghai(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Cancun(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Prague(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Osaka(bodies)) => ssz_response(bodies),
        Ok(PayloadBodies::Amsterdam(bodies)) => ssz_response(bodies),
        Err(err) => payload_bodies_error_response(err),
    }
}

enum PayloadBodies {
    Paris(BodiesResponseParis),
    Shanghai(BodiesResponseShanghai),
    Cancun(BodiesResponseCancun),
    Prague(BodiesResponsePrague),
    Osaka(BodiesResponseOsaka),
    Amsterdam(BodiesResponseAmsterdam),
}

#[derive(Debug)]
enum PayloadBodiesError {
    InvalidRange { start: BlockNumber, count: u64 },
    RequestTooLarge { len: u64 },
    Internal(String),
}

async fn get_payload_bodies_by_range<Provider>(
    provider: Provider,
    fork: EngineSszFork,
    start: BlockNumber,
    count: u64,
) -> Result<PayloadBodies, PayloadBodiesError>
where
    Provider: HeaderProvider + BlockReader + BalProvider + Send + 'static,
    Provider::Block: Send,
{
    let bal_store = provider.bal_store().clone();
    let payload_bodies = tokio::task::spawn_blocking(move || {
        if count > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(PayloadBodiesError::RequestTooLarge { len: count })
        }

        if start == 0 || count == 0 {
            return Err(PayloadBodiesError::InvalidRange { start, count })
        }

        let mut result = Vec::with_capacity(count as usize);
        let mut end = start.saturating_add(count - 1);

        if let Ok(best_block) = provider.best_block_number() &&
            end > best_block
        {
            end = best_block;
        }

        let earliest_block = provider.earliest_block_number().unwrap_or(0);
        for num in start..=end {
            if num < earliest_block {
                result.push(None);
                continue;
            }

            let block = provider
                .block(BlockHashOrNumber::Number(num))
                .map_err(|err| PayloadBodiesError::Internal(err.to_string()))?;
            result.push(block);
        }

        Ok(result)
    })
    .await
    .map_err(|err| PayloadBodiesError::Internal(err.to_string()))??;

    payload_bodies_response_with_bal_store(bal_store, fork, payload_bodies)
}

async fn get_payload_bodies_by_hash<Provider>(
    provider: Provider,
    fork: EngineSszFork,
    hashes: Vec<BlockHash>,
) -> Result<PayloadBodies, PayloadBodiesError>
where
    Provider: HeaderProvider + BlockReader + BalProvider + Send + 'static,
    Provider::Block: Send,
{
    let len = hashes.len() as u64;
    if len > MAX_PAYLOAD_BODIES_LIMIT {
        return Err(PayloadBodiesError::RequestTooLarge { len })
    }

    let bal_store = provider.bal_store().clone();
    let payload_bodies = tokio::task::spawn_blocking(move || {
        let mut result = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let block = provider
                .block(BlockHashOrNumber::Hash(hash))
                .map_err(|err| PayloadBodiesError::Internal(err.to_string()))?;
            result.push(block);
        }

        Ok(result)
    })
    .await
    .map_err(|err| PayloadBodiesError::Internal(err.to_string()))??;

    payload_bodies_response_with_bal_store(bal_store, fork, payload_bodies)
}

fn payload_bodies_response_with_bal_store<Block>(
    bal_store: reth_provider::BalStoreHandle,
    fork: EngineSszFork,
    payload_bodies: Vec<Option<Block>>,
) -> Result<PayloadBodies, PayloadBodiesError>
where
    Block: reth_primitives_traits::Block,
{
    match fork {
        EngineSszFork::Paris => {
            let entries = payload_bodies
                .into_iter()
                .map(|block| {
                    payload_body_entry::<ExecutionPayloadBodyParis, _>(block.map(payload_body_v1))
                })
                .collect();
            Ok(PayloadBodies::Paris(bodies_response(entries)?))
        }
        EngineSszFork::Shanghai => {
            let entries = payload_bodies
                .into_iter()
                .map(|block| {
                    payload_body_entry::<ExecutionPayloadBodyShanghai, _>(
                        block.map(payload_body_v1),
                    )
                })
                .collect();
            Ok(PayloadBodies::Shanghai(bodies_response(entries)?))
        }
        EngineSszFork::Cancun => {
            let entries = payload_bodies
                .into_iter()
                .map(|block| {
                    payload_body_entry::<ExecutionPayloadBodyCancun, _>(block.map(payload_body_v1))
                })
                .collect();
            Ok(PayloadBodies::Cancun(bodies_response(entries)?))
        }
        EngineSszFork::Prague => {
            let entries = payload_bodies
                .into_iter()
                .map(|block| {
                    payload_body_entry::<ExecutionPayloadBodyPrague, _>(block.map(payload_body_v1))
                })
                .collect();
            Ok(PayloadBodies::Prague(bodies_response(entries)?))
        }
        EngineSszFork::Osaka => {
            let entries = payload_bodies
                .into_iter()
                .map(|block| {
                    payload_body_entry::<ExecutionPayloadBodyOsaka, _>(block.map(payload_body_v1))
                })
                .collect();
            Ok(PayloadBodies::Osaka(bodies_response(entries)?))
        }
        EngineSszFork::Amsterdam => {
            let mut payload_bodies = payload_bodies
                .into_iter()
                .map(|block| {
                    block.map(|block| {
                        let block_hash = block.header().hash_slow();
                        (block_hash, payload_body_v2(block))
                    })
                })
                .collect::<Vec<_>>();
            let block_hashes = payload_bodies
                .iter()
                .filter_map(|payload_body| payload_body.as_ref().map(|(block_hash, _)| *block_hash))
                .collect::<Vec<_>>();
            let block_access_lists = bal_store
                .get_by_hashes(&block_hashes)
                .map_err(|err| PayloadBodiesError::Internal(err.to_string()))?;

            for (payload_body, block_access_list) in
                payload_bodies.iter_mut().filter_map(Option::as_mut).zip(block_access_lists)
            {
                payload_body.1.block_access_list = block_access_list;
            }

            let entries = payload_bodies
                .into_iter()
                .map(|payload_body| {
                    payload_body_entry::<ExecutionPayloadBodyAmsterdam, _>(
                        payload_body.map(|(_, payload_body)| payload_body),
                    )
                })
                .collect();

            Ok(PayloadBodies::Amsterdam(bodies_response(entries)?))
        }
    }
}

fn bodies_response<T>(entries: Vec<BodyEntry<T>>) -> Result<BodiesResponse<T>, PayloadBodiesError> {
    let len = entries.len() as u64;
    let entries = VariableList::<BodyEntry<T>, U32>::try_from(entries)
        .map_err(|_| PayloadBodiesError::RequestTooLarge { len })?;
    Ok(BodiesResponse { entries })
}

fn payload_body_entry<T, Legacy>(body: Option<Legacy>) -> BodyEntry<T>
where
    T: TryFrom<Legacy> + Default,
{
    match body.and_then(|body| T::try_from(body).ok()) {
        Some(body) => BodyEntry { available: true, body },
        None => BodyEntry { available: false, body: T::default() },
    }
}

fn payload_body_v1<Block>(block: Block) -> ExecutionPayloadBodyV1
where
    Block: reth_primitives_traits::Block,
{
    ExecutionPayloadBodyV1 {
        transactions: block.body().encoded_2718_transactions(),
        withdrawals: block.body().withdrawals().cloned().map(Withdrawals::into_inner),
    }
}

fn payload_body_v2<Block>(block: Block) -> ExecutionPayloadBodyV2
where
    Block: reth_primitives_traits::Block,
{
    ExecutionPayloadBodyV2 {
        transactions: block.body().encoded_2718_transactions(),
        withdrawals: block.body().withdrawals().cloned().map(Withdrawals::into_inner),
        block_access_list: None,
    }
}

fn payload_bodies_error_response(err: PayloadBodiesError) -> HttpResponse {
    match err {
        PayloadBodiesError::InvalidRange { start, count } => text_response(
            STATUS_BAD_REQUEST,
            format!("invalid bodies range: start={start}, count={count}"),
        ),
        PayloadBodiesError::RequestTooLarge { len } => text_response(
            STATUS_PAYLOAD_TOO_LARGE,
            format!("payload bodies request too large: {len}"),
        ),
        PayloadBodiesError::Internal(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err),
    }
}

/// Handles SSZ `engine_getBlobsV*` requests with the node's blob store.
async fn handle_get_blobs<ChainSpec, Provider>(
    handler: EngineSszProxyHandle<ChainSpec, Provider>,
    version: u8,
    body: &[u8],
) -> HttpResponse
where
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let Some(blob_store) = handler.blob_store().await else {
        return text_response(STATUS_SERVICE_UNAVAILABLE, "blob store unavailable")
    };

    let chain_spec = handler.chain_spec();
    let current_timestamp =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let unsupported_fork = || {
        text_response(STATUS_BAD_REQUEST, EngineObjectValidationError::UnsupportedFork.to_string())
    };

    match version {
        1 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            if chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
                return unsupported_fork()
            }
            if hashes.len() > MAX_BLOB_LIMIT {
                return text_response(
                    STATUS_PAYLOAD_TOO_LARGE,
                    format!("blob request too large: {}", hashes.len()),
                )
            }
            match blob_store.get_by_versioned_hashes_v1(&hashes) {
                Ok(response) => ssz_response(response),
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        2 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            if !chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
                return unsupported_fork()
            }
            if hashes.len() > MAX_BLOB_LIMIT {
                return text_response(
                    STATUS_PAYLOAD_TOO_LARGE,
                    format!("blob request too large: {}", hashes.len()),
                )
            }
            match blob_store.get_by_versioned_hashes_v2(&hashes) {
                Ok(Some(response)) => ssz_response(response),
                Ok(None) => no_content_response(),
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        3 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            if !chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
                return unsupported_fork()
            }
            if hashes.len() > MAX_BLOB_LIMIT {
                return text_response(
                    STATUS_PAYLOAD_TOO_LARGE,
                    format!("blob request too large: {}", hashes.len()),
                )
            }
            match blob_store.get_by_versioned_hashes_v3(&hashes) {
                Ok(response) => ssz_response(response),
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        4 => {
            let (hashes, indices_bitarray) = match decode_blob_cells_request(body) {
                Ok(request) => request,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            if !chain_spec.is_amsterdam_active_at_timestamp(current_timestamp) {
                return unsupported_fork()
            }
            if hashes.len() > MAX_BLOB_LIMIT {
                return text_response(
                    STATUS_PAYLOAD_TOO_LARGE,
                    format!("blob request too large: {}", hashes.len()),
                )
            }
            match blob_store.get_by_versioned_hashes_v4(&hashes, indices_bitarray) {
                Ok(response) => ssz_response(response),
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        _ => text_response(STATUS_NOT_FOUND, "unsupported blobs endpoint version"),
    }
}

/// Decodes the common getBlobs request container with only versioned hashes.
fn decode_blob_hashes_request(body: &[u8]) -> Result<Vec<B256>, &'static str> {
    Vec::<B256>::from_ssz_bytes(body).map_err(|_| "invalid ssz")
}

fn decode_payload_bodies_by_hash_request(body: &[u8]) -> Result<Vec<B256>, &'static str> {
    let request = BodiesByHashRequest::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
    Ok(request.block_hashes.into())
}

/// Decodes the Amsterdam getBlobs request container with hashes and a cell index mask.
fn decode_blob_cells_request(body: &[u8]) -> Result<(Vec<B256>, B128), &'static str> {
    <(Vec<B256>, B128) as ssz::Decode>::from_ssz_bytes(body).map_err(|_| "invalid ssz")
}

fn decode_new_payload_request(version: u8, body: &[u8]) -> Result<ExecutionData, &'static str> {
    match version {
        1 => {
            let execution_payload =
                decode_one::<ExecutionPayloadV1>(body).map_err(|_| "invalid ssz")?;
            Ok(ExecutionData::new(execution_payload.into(), ExecutionPayloadSidecar::none()))
        }
        2 => {
            let execution_payload =
                decode_one::<ExecutionPayloadV2>(body).map_err(|_| "invalid ssz")?;
            Ok(ExecutionData::new(execution_payload.into(), ExecutionPayloadSidecar::none()))
        }
        3 => {
            let (execution_payload, parent_beacon_block_root) =
                <(ExecutionPayloadV3, B256)>::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
                parent_beacon_block_root,
                versioned_hashes,
            });
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        4 => {
            let (execution_payload, parent_beacon_block_root, execution_requests) =
                <(ExecutionPayloadV3, B256, Vec<Bytes>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields { parent_beacon_block_root, versioned_hashes },
                PraguePayloadFields::new(RequestsOrHash::Requests(Requests::new(
                    execution_requests,
                ))),
            );
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        5 => {
            let (execution_payload, parent_beacon_block_root, execution_requests) =
                <(ExecutionPayloadV4, B256, Vec<Bytes>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields { parent_beacon_block_root, versioned_hashes },
                PraguePayloadFields::new(RequestsOrHash::Requests(Requests::new(
                    execution_requests,
                ))),
            );
            Ok(ExecutionData::new(ExecutionPayload::V4(execution_payload), sidecar))
        }
        _ => Err("unsupported payload endpoint version"),
    }
}

fn calculate_versioned_hashes(transactions: &[Bytes]) -> Result<Vec<B256>, &'static str> {
    let mut versioned_hashes = Vec::new();
    for transaction in transactions {
        let transaction =
            TxEnvelope::decode_2718_exact(transaction.as_ref()).map_err(|_| "invalid tx")?;
        if let Some(hashes) = transaction.blob_versioned_hashes() {
            versioned_hashes.extend_from_slice(hashes);
        }
    }

    Ok(versioned_hashes)
}

fn decode_forkchoice_request(
    version: u8,
    body: &[u8],
) -> Result<(ForkchoiceState, Option<PayloadAttributes>), &'static str> {
    match version {
        1..=4 => {
            let (forkchoice_state, payload_attributes) =
                <(ForkchoiceState, Vec<PayloadAttributes>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, payload_attrs(version, payload_attributes)?))
        }
        _ => Err("unsupported forkchoice endpoint version"),
    }
}

fn decode_one<T: ssz::Decode>(body: &[u8]) -> Result<T, ssz::DecodeError> {
    let mut builder = ssz::SszDecoderBuilder::new(body);
    builder.register_type::<T>()?;
    let mut decoder = builder.build()?;
    decoder.decode_next()
}

fn payload_attrs(
    version: u8,
    attrs: Vec<PayloadAttributes>,
) -> Result<Option<PayloadAttributes>, &'static str> {
    if attrs.len() > 1 {
        return Err("payload_attributes must contain at most one value")
    }

    attrs.into_iter().next().map(|attrs| validate_payload_attrs_version(version, attrs)).transpose()
}

fn validate_payload_attrs_version(
    version: u8,
    attrs: PayloadAttributes,
) -> Result<PayloadAttributes, &'static str> {
    let matches_version = match version {
        1 => {
            attrs.withdrawals.is_none() &&
                attrs.parent_beacon_block_root.is_none() &&
                attrs.slot_number.is_none()
        }
        2 => {
            attrs.withdrawals.is_some() &&
                attrs.parent_beacon_block_root.is_none() &&
                attrs.slot_number.is_none()
        }
        3 => {
            attrs.withdrawals.is_some() &&
                attrs.parent_beacon_block_root.is_some() &&
                attrs.slot_number.is_none()
        }
        4 => {
            attrs.withdrawals.is_some() &&
                attrs.parent_beacon_block_root.is_some() &&
                attrs.slot_number.is_some()
        }
        _ => false,
    };

    if matches_version {
        Ok(attrs)
    } else {
        Err("payload_attributes version does not match endpoint")
    }
}

fn ssz_response<T: ssz::Encode>(value: T) -> HttpResponse {
    HttpResponse::builder()
        .status(STATUS_OK)
        .header(CONTENT_TYPE, OCTET_STREAM)
        .body(HttpBody::from(value.as_ssz_bytes()))
        .expect("valid response")
}

fn no_content_response() -> HttpResponse {
    HttpResponse::builder().status(204).body(HttpBody::empty()).expect("valid response")
}

fn not_implemented_response(body: impl Into<String>) -> HttpResponse {
    text_response(STATUS_NOT_IMPLEMENTED, body)
}

fn text_response(status: u16, body: impl Into<String>) -> HttpResponse {
    HttpResponse::builder()
        .status(status)
        .header(CONTENT_TYPE, TEXT_PLAIN)
        .body(HttpBody::from(body.into()))
        .expect("valid response")
}

fn json_response(value: serde_json::Value) -> HttpResponse {
    HttpResponse::builder()
        .status(STATUS_OK)
        .header(CONTENT_TYPE, APPLICATION_JSON)
        .body(HttpBody::from(value.to_string()))
        .expect("valid response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::Encode;

    #[test]
    fn parses_capabilities_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/capabilities").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Capabilities);
    }

    #[test]
    fn parses_identity_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/identity").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Identity);
    }

    #[test]
    fn parses_fork_scoped_payload_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/prague/payloads").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Payloads(EngineSszFork::Prague));
    }

    #[test]
    fn parses_fork_scoped_forkchoice_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/amsterdam/forkchoice").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Forkchoice(EngineSszFork::Amsterdam));
    }

    #[test]
    fn parses_fork_scoped_payload_bodies_by_hash_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/cancun/bodies/hash").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::BodiesByHash(EngineSszFork::Cancun));
    }

    #[test]
    fn parses_fork_scoped_payload_bodies_by_range_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/shanghai/bodies").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::BodiesByRange(EngineSszFork::Shanghai));
    }

    #[test]
    fn rejects_legacy_version_scoped_endpoint() {
        assert!(parse_engine_path("/engine/v4/payloads").is_none());
    }

    #[test]
    fn rejects_payload_endpoint_with_extra_segments() {
        assert!(parse_engine_path("/engine/v2/prague/payloads/0x0102030405060708/extra").is_none());
    }

    #[test]
    fn parses_payload_bodies_range_query() {
        assert_eq!(parse_bodies_range_query(Some("from=1&count=128")), Some((1, 128)));
        assert_eq!(parse_bodies_range_query(Some("count=2&from=3")), Some((3, 2)));
    }

    #[test]
    fn rejects_invalid_payload_bodies_range_query() {
        assert!(parse_bodies_range_query(Some("from=1")).is_none());
        assert!(parse_bodies_range_query(Some("from=1&count=2&extra=3")).is_none());
        assert!(parse_bodies_range_query(Some("from=one&count=2")).is_none());
        assert!(parse_bodies_range_query(None).is_none());
    }

    #[test]
    fn decodes_top_level_blob_hashes_request() {
        let hashes = vec![B256::ZERO, B256::with_last_byte(1)];
        let decoded = decode_blob_hashes_request(&hashes.as_ssz_bytes()).unwrap();
        assert_eq!(decoded, hashes);
    }
}
