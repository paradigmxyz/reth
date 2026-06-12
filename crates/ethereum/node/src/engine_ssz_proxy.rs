//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v2`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{
    eip2718::Decodable2718,
    eip7685::{Requests, RequestsOrHash},
};
use alloy_primitives::{Bytes, B128, B256};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ClientVersionV1, ExecutionData, ExecutionPayload,
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadEnvelopeV5, ExecutionPayloadEnvelopeV6, ExecutionPayloadSidecar,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ExecutionPayloadV4,
    ForkchoiceState, PayloadAttributes, PayloadId, PraguePayloadFields,
};
use http_body_util::BodyExt;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes};
use reth_node_core::version::{version_metadata, CLIENT_CODE};
use reth_payload_builder::PayloadStore;
use reth_payload_primitives::EngineObjectValidationError;
use reth_transaction_pool::BlobStore;
use ssz::Decode;
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
const CACHE_CONTROL: &str = "cache-control";
const NO_STORE: &str = "no-store";

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
const STATUS_PAYLOAD_TOO_LARGE: u16 = 413;
const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_SERVICE_UNAVAILABLE: u16 = 503;

const MAX_BLOB_LIMIT: usize = 128;
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// Shared handle used by [`EngineSszProxyLayer`].
pub struct EngineSszProxyHandle<ChainSpec> {
    engine: Arc<RwLock<Option<ConsensusEngineHandle<EthEngineTypes>>>>,
    payload_store: Arc<RwLock<Option<Arc<PayloadStore<EthEngineTypes>>>>>,
    blob_store: Arc<RwLock<Option<Arc<dyn BlobStore>>>>,
    client_version: Arc<RwLock<Option<ClientVersionV1>>>,
    chain_spec: Arc<ChainSpec>,
}

impl<C> Clone for EngineSszProxyHandle<C> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            payload_store: self.payload_store.clone(),
            blob_store: self.blob_store.clone(),
            client_version: self.client_version.clone(),
            chain_spec: self.chain_spec.clone(),
        }
    }
}

impl<C> std::fmt::Debug for EngineSszProxyHandle<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSszProxyHandle").finish_non_exhaustive()
    }
}

impl<ChainSpec> EngineSszProxyHandle<ChainSpec> {
    fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            engine: Default::default(),
            payload_store: Default::default(),
            blob_store: Default::default(),
            client_version: Arc::new(RwLock::new(Some(default_client_version()))),
            chain_spec,
        }
    }

    /// Sets the consensus engine handle used by the proxy.
    pub async fn set_engine(&self, engine: ConsensusEngineHandle<EthEngineTypes>) {
        *self.engine.write().await = Some(engine);
    }

    /// Sets the payload store used by the proxy.
    pub async fn set_payload_store(&self, payload_store: PayloadStore<EthEngineTypes>) {
        *self.payload_store.write().await = Some(Arc::new(payload_store));
    }

    async fn engine(&self) -> Option<ConsensusEngineHandle<EthEngineTypes>> {
        self.engine.read().await.clone()
    }

    async fn payload_store(&self) -> Option<Arc<PayloadStore<EthEngineTypes>>> {
        self.payload_store.read().await.clone()
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

    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec.clone()
    }
}

/// A tower layer that intercepts SSZ Engine API routes under `/engine/v2`.
#[derive(Clone, Debug)]
pub struct EngineSszProxyLayer<ChainSpec> {
    handle: EngineSszProxyHandle<ChainSpec>,
}

impl<ChainSpec> EngineSszProxyLayer<ChainSpec> {
    /// Creates a new proxy layer and a handle for setting the engine after node launch.
    pub fn new(chain_spec: Arc<ChainSpec>) -> (Self, EngineSszProxyHandle<ChainSpec>) {
        let handle = EngineSszProxyHandle::new(chain_spec);
        (Self { handle: handle.clone() }, handle)
    }
}

impl<S, ChainSpec> Layer<S> for EngineSszProxyLayer<ChainSpec> {
    type Service = EngineSszProxyService<S, ChainSpec>;

    fn layer(&self, inner: S) -> Self::Service {
        EngineSszProxyService { inner, handle: self.handle.clone() }
    }
}

/// The service produced by [`EngineSszProxyLayer`].
#[derive(Clone, Debug)]
pub struct EngineSszProxyService<S, ChainSpec> {
    inner: S,
    handle: EngineSszProxyHandle<ChainSpec>,
}

impl<S, ChainSpec> Service<HttpRequest> for EngineSszProxyService<S, ChainSpec>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError> + Send + Clone,
    S::Future: Send + 'static,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: HttpRequest) -> Self::Future {
        if !request.uri().path().starts_with("/engine/v2/") {
            let fut = self.inner.call(request);
            return Box::pin(fut)
        }

        let handle = self.handle.clone();
        Box::pin(async move { Ok(handle_engine_ssz_request(handle, request).await) })
    }
}

async fn handle_engine_ssz_request<ChainSpec>(
    handle: EngineSszProxyHandle<ChainSpec>,
    request: HttpRequest,
) -> HttpResponse
where
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
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
        EngineSszEndpoint::NewPayload(fork) => {
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
        EngineSszEndpoint::GetPayload(fork, payload_id) => {
            if method != "GET" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let response = if let Some(payload_id) = parse_payload_id(&payload_id) {
                if let Some(payload_store) = handle.payload_store().await {
                    handle_get_payload(payload_store, fork.get_payload_version(), payload_id).await
                } else {
                    text_response(STATUS_SERVICE_UNAVAILABLE, "payload store unavailable")
                }
            } else {
                text_response(STATUS_BAD_REQUEST, "invalid payload id")
            };
            no_store_response(response)
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
            Some(EngineSszEndpoint::NewPayload(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("payloads"), Some(payload_id), None) => {
            Some(EngineSszEndpoint::GetPayload(fork.parse().ok()?, payload_id.to_string()))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("forkchoice"), None, None) => {
            Some(EngineSszEndpoint::Forkchoice(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some("blobs"), version, None, None) => {
            Some(EngineSszEndpoint::Blobs(parse_method_version(version?)?))
        }
        _ => None,
    }
}

fn parse_payload_id(value: &str) -> Option<PayloadId> {
    let value = value.strip_prefix("0x")?;
    if value.len() != 16 {
        return None
    }

    let mut bytes = [0u8; 8];
    for (idx, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[idx * 2..idx * 2 + 2], 16).ok()?;
    }
    Some(PayloadId::new(bytes))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EngineSszEndpoint {
    Capabilities,
    Identity,
    NewPayload(EngineSszFork),
    GetPayload(EngineSszFork, String),
    Forkchoice(EngineSszFork),
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

    const fn get_payload_version(self) -> u8 {
        match self {
            Self::Paris => 1,
            Self::Shanghai => 2,
            Self::Cancun => 3,
            Self::Prague => 4,
            Self::Osaka => 5,
            Self::Amsterdam => 6,
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
            "bodies.max_count": 128,
            "blobs.max_versioned_hashes": MAX_BLOB_LIMIT,
            "payload.max_bytes": MAX_PAYLOAD_BYTES,
        },
    }))
}

fn handle_identity(client_version: Option<ClientVersionV1>) -> HttpResponse {
    json_response(client_version.into_iter().collect::<Vec<_>>())
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

async fn handle_get_payload(
    payload_store: Arc<PayloadStore<EthEngineTypes>>,
    version: u8,
    payload_id: PayloadId,
) -> HttpResponse {
    let payload = match payload_store.resolve(payload_id).await {
        Some(Ok(payload)) => payload,
        Some(Err(err)) => return text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        None => return text_response(STATUS_NOT_FOUND, "unknown payload"),
    };

    encode_get_payload_response(version, payload)
}

fn encode_get_payload_response(version: u8, payload: EthBuiltPayload) -> HttpResponse {
    match version {
        1 => ssz_response(ExecutionPayloadV1::from(payload)),
        2 => ssz_response(ExecutionPayloadEnvelopeV2::from(payload)),
        3 => match ExecutionPayloadEnvelopeV3::try_from(payload) {
            Ok(payload) => ssz_response(payload),
            Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        },
        4 => match ExecutionPayloadEnvelopeV4::try_from(payload) {
            Ok(payload) => ssz_response(payload),
            Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        },
        5 => match ExecutionPayloadEnvelopeV5::try_from(payload) {
            Ok(payload) => ssz_response(payload),
            Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        },
        6 => match ExecutionPayloadEnvelopeV6::try_from(payload) {
            Ok(payload) => ssz_response(payload),
            Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        },
        _ => text_response(STATUS_BAD_REQUEST, "unsupported getPayload endpoint version"),
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

/// Handles SSZ `engine_getBlobsV*` requests with the node's blob store.
async fn handle_get_blobs<ChainSpec>(
    handler: EngineSszProxyHandle<ChainSpec>,
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
        1..=3 => {
            let (forkchoice_state, payload_attributes) =
                <(ForkchoiceState, Vec<PayloadAttributes>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, payload_attrs(version, payload_attributes)?))
        }
        4 => {
            let (forkchoice_state, payload_attributes, custody_columns) =
                <(ForkchoiceState, Vec<PayloadAttributes>, Vec<B128>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            custody_columns_opt(custody_columns)?;
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

fn custody_columns_opt(custody_columns: Vec<B128>) -> Result<Option<B128>, &'static str> {
    if custody_columns.len() > 1 {
        return Err("invalid params")
    }

    Ok(custody_columns.into_iter().next())
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

fn json_response<T: serde::Serialize>(value: T) -> HttpResponse {
    let Ok(body) = serde_json::to_string(&value) else {
        return text_response(STATUS_INTERNAL_SERVER_ERROR, "failed to encode json")
    };

    HttpResponse::builder()
        .status(STATUS_OK)
        .header(CONTENT_TYPE, APPLICATION_JSON)
        .body(HttpBody::from(body))
        .expect("valid response")
}

fn no_content_response() -> HttpResponse {
    HttpResponse::builder().status(204).body(HttpBody::empty()).expect("valid response")
}

fn no_store_response(mut response: HttpResponse) -> HttpResponse {
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE.parse().expect("valid header value"));
    response
}

fn text_response(status: u16, body: impl Into<String>) -> HttpResponse {
    HttpResponse::builder()
        .status(status)
        .header(CONTENT_TYPE, TEXT_PLAIN)
        .body(HttpBody::from(body.into()))
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
        assert_eq!(endpoint, EngineSszEndpoint::NewPayload(EngineSszFork::Prague));
    }

    #[test]
    fn parses_fork_scoped_get_payload_endpoint() {
        let EngineSszEndpoint::GetPayload(fork, payload_id) =
            parse_engine_path("/engine/v2/prague/payloads/0x1234567890abcdef").unwrap()
        else {
            panic!("expected get payload route")
        };
        assert_eq!(fork, EngineSszFork::Prague);
        assert_eq!(fork.get_payload_version(), 4);
        assert_eq!(
            parse_payload_id(&payload_id),
            Some(PayloadId::new([0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]))
        );
    }

    #[test]
    fn parses_get_payload_route_with_malformed_id() {
        let endpoint = parse_engine_path("/engine/v2/prague/payloads/not-a-payload-id").unwrap();
        assert_eq!(
            endpoint,
            EngineSszEndpoint::GetPayload(EngineSszFork::Prague, "not-a-payload-id".to_string())
        );
    }

    #[test]
    fn adds_no_store_header() {
        let response = no_store_response(text_response(STATUS_NOT_FOUND, "unknown payload"));
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), NO_STORE);
    }

    #[test]
    fn rejects_get_payload_endpoint_with_extra_segments() {
        assert!(parse_engine_path("/engine/v2/prague/payloads/0x1234567890abcdef/extra").is_none());
    }

    #[test]
    fn parses_fork_scoped_forkchoice_endpoint() {
        let endpoint = parse_engine_path("/engine/v2/amsterdam/forkchoice").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Forkchoice(EngineSszFork::Amsterdam));
    }

    #[test]
    fn rejects_legacy_version_scoped_endpoint() {
        assert!(parse_engine_path("/engine/v4/payloads").is_none());
    }

    #[test]
    fn decodes_top_level_blob_hashes_request() {
        let hashes = vec![B256::ZERO, B256::with_last_byte(1)];
        let decoded = decode_blob_hashes_request(&hashes.as_ssz_bytes()).unwrap();
        assert_eq!(decoded, hashes);
    }

    #[test]
    fn decodes_forkchoice_v4_with_custody_columns() {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: B256::ZERO,
            safe_block_hash: B256::ZERO,
            finalized_block_hash: B256::ZERO,
        };
        let encoded =
            (forkchoice_state, Vec::<PayloadAttributes>::new(), vec![B128::with_last_byte(1)])
                .as_ssz_bytes();

        let (decoded_state, decoded_attrs) = decode_forkchoice_request(4, &encoded).unwrap();
        assert_eq!(decoded_state, forkchoice_state);
        assert!(decoded_attrs.is_none());
    }

    #[test]
    fn rejects_forkchoice_v4_with_multiple_custody_columns() {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: B256::ZERO,
            safe_block_hash: B256::ZERO,
            finalized_block_hash: B256::ZERO,
        };
        let encoded = (
            forkchoice_state,
            Vec::<PayloadAttributes>::new(),
            vec![B128::ZERO, B128::with_last_byte(1)],
        )
            .as_ssz_bytes();

        let err = decode_forkchoice_request(4, &encoded).unwrap_err();
        assert_eq!(err, "invalid params");
    }
}
