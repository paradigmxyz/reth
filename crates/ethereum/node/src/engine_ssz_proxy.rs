//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v2`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{eip2718::Decodable2718, eip7685::RequestsOrHash};
use alloy_primitives::{Bytes, B128, B256};
use alloy_rpc_types_engine::{
    ssz_engine_types::{
        BlobsV1Request, BlobsV1Response, BlobsV2Response, BlobsV3Response, BlobsV4Request,
        BlobsV4Response, ExecutionPayloadEnvelopeAmsterdam, ExecutionPayloadEnvelopeCancun,
        ExecutionPayloadEnvelopeOsaka, ExecutionPayloadEnvelopeParis,
        ExecutionPayloadEnvelopePrague, ExecutionPayloadEnvelopeShanghai,
        ForkchoiceUpdateAmsterdam, ForkchoiceUpdateCancun, ForkchoiceUpdateOsaka,
        ForkchoiceUpdateParis, ForkchoiceUpdatePrague, ForkchoiceUpdateResponse,
        ForkchoiceUpdateShanghai, PayloadStatus as SszPayloadStatus,
    },
    CancunPayloadFields, ClientVersionV1, ExecutionData, ExecutionPayload, ExecutionPayloadSidecar,
    ForkchoiceState, PayloadAttributes, PraguePayloadFields,
};
use http_body_util::BodyExt;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_core::version::{version_metadata, CLIENT_CODE};
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
    blob_store: Arc<RwLock<Option<Arc<dyn BlobStore>>>>,
    client_version: Arc<RwLock<Option<ClientVersionV1>>>,
    chain_spec: Arc<ChainSpec>,
}

impl<C> Clone for EngineSszProxyHandle<C> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
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
            blob_store: Default::default(),
            client_version: Arc::new(RwLock::new(Some(default_client_version()))),
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
        if !request.uri().path().starts_with("/engine/") {
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
            handle_new_payload(engine, fork, &body).await
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
            handle_forkchoice_updated(engine, fork, &body).await
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
    match (segments.next(), segments.next(), segments.next(), segments.next(), segments.next()) {
        (Some("engine"), Some("v2"), Some("capabilities"), None, None) => {
            Some(EngineSszEndpoint::Capabilities)
        }
        (Some("engine"), Some("v2"), Some("identity"), None, None) => {
            Some(EngineSszEndpoint::Identity)
        }
        (Some("engine"), Some("v2"), Some(fork), Some("payloads"), None) => {
            Some(EngineSszEndpoint::Payloads(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("forkchoice"), None) => {
            Some(EngineSszEndpoint::Forkchoice(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some("blobs"), version, None) => {
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
    fork: EngineSszFork,
    body: &[u8],
) -> HttpResponse {
    let payload = match decode_new_payload_request(fork, body) {
        Ok(payload) => payload,
        Err(err) => return text_response(STATUS_BAD_REQUEST, err),
    };

    match engine.new_payload(payload).await {
        Ok(status) => match SszPayloadStatus::try_from(status) {
            Ok(status) => ssz_response(status),
            Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        },
        Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn handle_forkchoice_updated(
    engine: ConsensusEngineHandle<EthEngineTypes>,
    fork: EngineSszFork,
    body: &[u8],
) -> HttpResponse {
    let (state, attrs) = match decode_forkchoice_request(fork, body) {
        Ok(request) => request,
        Err(err) => return text_response(STATUS_BAD_REQUEST, err),
    };

    match engine.fork_choice_updated(state, attrs).await {
        Ok(updated) => match ForkchoiceUpdateResponse::try_from(updated) {
            Ok(updated) => ssz_response(updated),
            Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
        },
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
                Ok(response) => match BlobsV1Response::try_from(response) {
                    Ok(response) => ssz_response(response),
                    Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
                },
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
                Ok(Some(response)) => match BlobsV2Response::try_from(response) {
                    Ok(response) => ssz_response(response),
                    Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
                },
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
                Ok(response) => match BlobsV3Response::try_from(response) {
                    Ok(response) => ssz_response(response),
                    Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
                },
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
                Ok(response) => match BlobsV4Response::try_from(response) {
                    Ok(response) => ssz_response(response),
                    Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
                },
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        _ => text_response(STATUS_NOT_FOUND, "unsupported blobs endpoint version"),
    }
}

/// Decodes the common getBlobs request container with only versioned hashes.
fn decode_blob_hashes_request(body: &[u8]) -> Result<Vec<B256>, &'static str> {
    BlobsV1Request::from_ssz_bytes(body)
        .map(|request| request.versioned_hashes.into())
        .map_err(|_| "invalid ssz")
}

/// Decodes the Amsterdam getBlobs request container with hashes and a cell index mask.
fn decode_blob_cells_request(body: &[u8]) -> Result<(Vec<B256>, B128), &'static str> {
    BlobsV4Request::from_ssz_bytes(body)
        .map(|request| (request.versioned_hashes.into(), request.indices_bitarray))
        .map_err(|_| "invalid ssz")
}

fn decode_new_payload_request(
    fork: EngineSszFork,
    body: &[u8],
) -> Result<ExecutionData, &'static str> {
    match fork {
        EngineSszFork::Paris => {
            let ExecutionPayloadEnvelopeParis { payload: execution_payload } =
                ExecutionPayloadEnvelopeParis::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok(ExecutionData::new(execution_payload.into(), ExecutionPayloadSidecar::none()))
        }
        EngineSszFork::Shanghai => {
            let ExecutionPayloadEnvelopeShanghai { payload: execution_payload } =
                ExecutionPayloadEnvelopeShanghai::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            Ok(ExecutionData::new(execution_payload.into(), ExecutionPayloadSidecar::none()))
        }
        EngineSszFork::Cancun => {
            let ExecutionPayloadEnvelopeCancun {
                payload: execution_payload,
                parent_beacon_block_root,
            } = ExecutionPayloadEnvelopeCancun::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
                parent_beacon_block_root,
                versioned_hashes,
            });
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        EngineSszFork::Prague => {
            let ExecutionPayloadEnvelopePrague {
                payload: execution_payload,
                parent_beacon_block_root,
                execution_requests,
            } = ExecutionPayloadEnvelopePrague::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields { parent_beacon_block_root, versioned_hashes },
                PraguePayloadFields::new(RequestsOrHash::Requests(execution_requests)),
            );
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        EngineSszFork::Osaka => {
            let ExecutionPayloadEnvelopeOsaka {
                payload: execution_payload,
                parent_beacon_block_root,
                execution_requests,
            } = ExecutionPayloadEnvelopeOsaka::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields { parent_beacon_block_root, versioned_hashes },
                PraguePayloadFields::new(RequestsOrHash::Requests(execution_requests)),
            );
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        EngineSszFork::Amsterdam => {
            let ExecutionPayloadEnvelopeAmsterdam {
                payload: execution_payload,
                parent_beacon_block_root,
                execution_requests,
            } = ExecutionPayloadEnvelopeAmsterdam::from_ssz_bytes(body)
                .map_err(|_| "invalid ssz")?;
            let versioned_hashes = calculate_versioned_hashes(
                &execution_payload.payload_inner.payload_inner.payload_inner.transactions,
            )?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields { parent_beacon_block_root, versioned_hashes },
                PraguePayloadFields::new(RequestsOrHash::Requests(execution_requests)),
            );
            Ok(ExecutionData::new(ExecutionPayload::V4(execution_payload), sidecar))
        }
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
    fork: EngineSszFork,
    body: &[u8],
) -> Result<(ForkchoiceState, Option<PayloadAttributes>), &'static str> {
    match fork {
        EngineSszFork::Paris => {
            let request = ForkchoiceUpdateParis::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((request.forkchoice_state, request.payload_attributes.into_option().map(Into::into)))
        }
        EngineSszFork::Shanghai => {
            let request =
                ForkchoiceUpdateShanghai::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((request.forkchoice_state, request.payload_attributes.into_option().map(Into::into)))
        }
        EngineSszFork::Cancun => {
            let request =
                ForkchoiceUpdateCancun::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((request.forkchoice_state, request.payload_attributes.into_option().map(Into::into)))
        }
        EngineSszFork::Prague => {
            let request =
                ForkchoiceUpdatePrague::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((request.forkchoice_state, request.payload_attributes.into_option().map(Into::into)))
        }
        EngineSszFork::Osaka => {
            let request = ForkchoiceUpdateOsaka::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((request.forkchoice_state, request.payload_attributes.into_option().map(Into::into)))
        }
        EngineSszFork::Amsterdam => {
            let request =
                ForkchoiceUpdateAmsterdam::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((request.forkchoice_state, request.payload_attributes.into_option().map(Into::into)))
        }
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
    use alloy_rpc_types_engine::ssz_engine_types::Optional;
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
    fn rejects_legacy_version_scoped_endpoint() {
        assert!(parse_engine_path("/engine/v4/payloads").is_none());
    }

    #[test]
    fn decodes_blob_hashes_request_container() {
        let hashes = vec![B256::ZERO, B256::with_last_byte(1)];
        let request = BlobsV1Request { versioned_hashes: hashes.clone().try_into().unwrap() };
        let decoded = decode_blob_hashes_request(&request.as_ssz_bytes()).unwrap();
        assert_eq!(decoded, hashes);
    }

    #[test]
    fn decodes_forkchoice_v4_with_custody_columns() {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: B256::ZERO,
            safe_block_hash: B256::ZERO,
            finalized_block_hash: B256::ZERO,
        };
        let encoded = ForkchoiceUpdateAmsterdam {
            forkchoice_state,
            payload_attributes: Optional::none(),
            custody_columns: Optional::some(B128::with_last_byte(1)),
        }
        .as_ssz_bytes();

        let (decoded_state, decoded_attrs) =
            decode_forkchoice_request(EngineSszFork::Amsterdam, &encoded).unwrap();
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

        let err = decode_forkchoice_request(EngineSszFork::Amsterdam, &encoded).unwrap_err();
        assert_eq!(err, "invalid ssz");
    }
}
