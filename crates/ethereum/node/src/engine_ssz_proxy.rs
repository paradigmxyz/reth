//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v1`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

pub use crate::engine_ssz_witness::{EngineSszWitness, EngineSszWitnessGenerator};

use crate::engine_ssz_containers::{
    BlobsV1Request, BlobsV1Response, BlobsV2Response, BlobsV3Response, BlobsV4Request,
    BlobsV4Response, BodiesByHashRequest, BodiesResponse, BuiltPayloadAmsterdam,
    BuiltPayloadCancun, BuiltPayloadOsaka, BuiltPayloadParis, BuiltPayloadPrague,
    BuiltPayloadShanghai, ExecutionPayloadBodyAmsterdam, ExecutionPayloadBodyParis,
    ExecutionPayloadBodyShanghai, ExecutionPayloadEnvelopeAmsterdam,
    ExecutionPayloadEnvelopeCancun, ExecutionPayloadEnvelopeOsaka, ExecutionPayloadEnvelopeParis,
    ExecutionPayloadEnvelopePrague, ExecutionPayloadEnvelopeShanghai, ForkchoiceUpdateAmsterdam,
    ForkchoiceUpdateCancun, ForkchoiceUpdateOsaka, ForkchoiceUpdateParis, ForkchoiceUpdatePrague,
    ForkchoiceUpdateResponse, ForkchoiceUpdateShanghai, Optional,
    PayloadStatus as EngineSszPayloadStatus, PayloadStatusWithWitness,
};
use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{eip2718::Decodable2718, eip7685::RequestsOrHash};
use alloy_primitives::{Bytes, B128, B256};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadBodyV1,
    ExecutionPayloadFieldV2, ExecutionPayloadSidecar, ForkchoiceState, PayloadAttributes,
    PayloadId, PraguePayloadFields,
};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_chainspec::{EthereumHardfork, EthereumHardforks};
use reth_engine_primitives::EngineApiValidator;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_provider::{BalProvider, BlockReader, HeaderProvider, StateProviderFactory};
use reth_rpc::EngineApi;
use reth_rpc_engine_api::EngineApiError;
use reth_transaction_pool::TransactionPool;
use ssz::Decode;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::RwLock;
use tower::{BoxError, Layer, Service};

const OCTET_STREAM: &str = "application/octet-stream";
const APPLICATION_JSON: &str = "application/json";
const CONTENT_TYPE: &str = "content-type";
const CACHE_CONTROL: &str = "cache-control";
const ETH_EXECUTION_VERSION: &str = "eth-execution-version";

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
const STATUS_PAYLOAD_TOO_LARGE: u16 = 413;
const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_SERVICE_UNAVAILABLE: u16 = 503;
const STATUS_UNSUPPORTED_MEDIA_TYPE: u16 = 415;

const MAX_BLOB_LIMIT: usize = 128;
const MAX_BLOB_REQUEST_BYTES: u64 = 4 + 16 + MAX_BLOB_LIMIT as u64 * 32;
const MAX_BODIES_REQUEST: usize = crate::engine_ssz_containers::MAX_BODIES_REQUEST;
const MAX_BODIES_REQUEST_BYTES: u64 = 4 + (MAX_BODIES_REQUEST as u64 * 32);
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
const PROBLEM_JSON: &str = "application/problem+json";

type EthEngineApi<Provider, Pool, Validator, ChainSpec> =
    EngineApi<Provider, EthEngineTypes, Pool, Validator, ChainSpec>;
type SharedEngineApi<Api> = Arc<RwLock<Option<Api>>>;
type SharedWitnessHandler = Arc<RwLock<Option<Arc<dyn EngineSszWitness>>>>;

/// Shared handle used by [`EngineSszProxyLayer`].
pub struct EngineSszProxyHandle<Api = ()> {
    engine_api: SharedEngineApi<Api>,
    witness_handler: SharedWitnessHandler,
}

impl<Api> Clone for EngineSszProxyHandle<Api> {
    fn clone(&self) -> Self {
        Self { engine_api: self.engine_api.clone(), witness_handler: self.witness_handler.clone() }
    }
}

impl<Api> std::fmt::Debug for EngineSszProxyHandle<Api> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSszProxyHandle").finish_non_exhaustive()
    }
}

impl<Api> EngineSszProxyHandle<Api> {
    fn new() -> Self {
        Self { engine_api: Default::default(), witness_handler: Default::default() }
    }

    fn with_engine_api(engine_api: Api) -> Self {
        Self {
            engine_api: Arc::new(RwLock::new(Some(engine_api))),
            witness_handler: Default::default(),
        }
    }

    /// Sets the Engine API implementation used by the proxy.
    pub async fn set_engine_api(&self, engine_api: Api) {
        *self.engine_api.write().await = Some(engine_api);
    }

    /// Sets the Engine API implementation during synchronous launch wiring.
    pub fn set_engine_api_sync(&self, engine_api: Api) {
        *self
            .engine_api
            .try_write()
            .expect("engine api handle should not be locked during launch") = Some(engine_api);
    }

    /// Sets the witness generator used by `/payloads/witness`.
    pub async fn set_witness_handler(&self, witness_handler: Arc<dyn EngineSszWitness>) {
        *self.witness_handler.write().await = Some(witness_handler);
    }

    /// Sets the witness generator during synchronous launch wiring.
    pub fn set_witness_handler_sync(&self, witness_handler: Arc<dyn EngineSszWitness>) {
        *self
            .witness_handler
            .try_write()
            .expect("witness handler should not be locked during launch") = Some(witness_handler);
    }
}

impl<Api: Clone> EngineSszProxyHandle<Api> {
    /// Returns the witness generator used by `/payloads/witness`.
    pub async fn witness_handler(&self) -> Option<Arc<dyn EngineSszWitness>> {
        self.witness_handler.read().await.clone()
    }

    /// Returns the Engine API implementation used by the proxy.
    pub async fn engine_api(&self) -> Option<Api> {
        self.engine_api.read().await.clone()
    }
}

/// A tower layer that intercepts SSZ Engine API routes under `/engine/v1`.
#[derive(Clone, Debug)]
pub struct EngineSszProxyLayer<Api = ()> {
    handle: EngineSszProxyHandle<Api>,
}

impl<Api> EngineSszProxyLayer<Api> {
    /// Creates a new proxy layer and a handle for setting the engine after node launch.
    pub fn new() -> (Self, EngineSszProxyHandle<Api>) {
        let handle = EngineSszProxyHandle::new();
        (Self { handle: handle.clone() }, handle)
    }

    /// Creates a new proxy layer with an Engine API implementation.
    pub fn with_engine_api(engine_api: Api) -> (Self, EngineSszProxyHandle<Api>) {
        let handle = EngineSszProxyHandle::with_engine_api(engine_api);
        (Self { handle: handle.clone() }, handle)
    }
}

impl<S, Api> Layer<S> for EngineSszProxyLayer<Api> {
    type Service = EngineSszProxyService<S, Api>;

    fn layer(&self, inner: S) -> Self::Service {
        EngineSszProxyService { inner, handle: self.handle.clone() }
    }
}

/// The service produced by [`EngineSszProxyLayer`].
#[derive(Clone, Debug)]
pub struct EngineSszProxyService<S, Api = ()> {
    inner: S,
    handle: EngineSszProxyHandle<Api>,
}

impl<S, Api> Service<HttpRequest> for EngineSszProxyService<S, Api>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError> + Send + Clone,
    S::Future: Send + 'static,
    Api: EngineSszApi,
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

async fn handle_engine_ssz_request<Api>(
    handle: EngineSszProxyHandle<Api>,
    request: HttpRequest,
) -> HttpResponse
where
    Api: EngineSszApi,
{
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let Some(endpoint) = parse_engine_path(&path) else {
        return problem_response(STATUS_NOT_FOUND, "method-not-found", None)
    };

    match endpoint {
        EngineSszEndpoint::Capabilities => {
            if method != "GET" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            handle_capabilities(
                handle.witness_handler().await.is_some() &&
                    handle.engine_api().await.is_some_and(|api| api.supports_witness()),
            )
        }
        EngineSszEndpoint::Identity => {
            if method != "GET" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.identity()
        }
        EngineSszEndpoint::NewPayload => {
            if method != "POST" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            let body = match read_ssz_body(request, MAX_PAYLOAD_BYTES).await {
                Ok(body) => body,
                Err(response) => return response,
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.new_payload(fork, body).await
        }
        EngineSszEndpoint::PayloadsWithWitness => {
            if method != "POST" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            if fork != EngineSszFork::Amsterdam {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            }
            let body = match read_ssz_body(request, MAX_PAYLOAD_BYTES).await {
                Ok(body) => body,
                Err(response) => return response,
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            let Some(witness_handler) = handle.witness_handler().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.new_payload_with_witness(body, witness_handler).await
        }
        EngineSszEndpoint::GetPayload(payload_id) => {
            if method != "GET" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Ok(payload_id) = payload_id else {
                return problem_response(STATUS_BAD_REQUEST, "invalid-request", None)
            };
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.get_payload(fork, payload_id).await
        }
        EngineSszEndpoint::Forkchoice => {
            if method != "POST" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            let body = match read_ssz_body(request, MAX_PAYLOAD_BYTES).await {
                Ok(body) => body,
                Err(response) => return response,
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.forkchoice_updated(fork, body).await
        }
        EngineSszEndpoint::PayloadBodiesByHash => {
            if method != "POST" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            let body = match read_ssz_body(request, MAX_BODIES_REQUEST_BYTES).await {
                Ok(body) => body,
                Err(response) => return response,
            };
            let request = match BodiesByHashRequest::from_ssz_bytes(&body) {
                Ok(request) if request.block_hashes.len() <= MAX_BODIES_REQUEST => {
                    request.block_hashes
                }
                Ok(_) => {
                    return problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None)
                }
                Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.get_payload_bodies_by_hash(fork, request).await
        }
        EngineSszEndpoint::PayloadBodiesByRange => {
            if method != "GET" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            let Some(query) = request.uri().query() else {
                return problem_response(
                    STATUS_BAD_REQUEST,
                    "invalid-request",
                    Some("missing payload bodies query".to_string()),
                )
            };
            let mut start = None;
            let mut count = None;
            for pair in query.split('&') {
                let Some((key, value)) = pair.split_once('=') else {
                    return problem_response(STATUS_BAD_REQUEST, "invalid-request", None)
                };
                match key {
                    "from" => match value.parse() {
                        Ok(value) => start = Some(value),
                        Err(_) => {
                            return problem_response(STATUS_BAD_REQUEST, "invalid-request", None)
                        }
                    },
                    "count" => match value.parse() {
                        Ok(value) => count = Some(value),
                        Err(_) => {
                            return problem_response(STATUS_BAD_REQUEST, "invalid-request", None)
                        }
                    },
                    _ => return problem_response(STATUS_BAD_REQUEST, "invalid-request", None),
                }
            }
            let (Some(start), Some(count)) = (start, count) else {
                return problem_response(STATUS_BAD_REQUEST, "invalid-request", None)
            };
            if count > MAX_BODIES_REQUEST as u64 {
                return problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None)
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.get_payload_bodies_by_range(fork, start, count).await
        }
        EngineSszEndpoint::Blobs(version) => {
            if method != "POST" {
                return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
            }
            let body = match read_ssz_body(request, MAX_BLOB_REQUEST_BYTES).await {
                Ok(body) => body,
                Err(response) => return response,
            };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.get_blobs(version, body).await
        }
    }
}

fn request_fork(request: &HttpRequest) -> Option<EngineSszFork> {
    request.headers().get(ETH_EXECUTION_VERSION)?.to_str().ok()?.parse().ok()
}

fn parse_engine_path(path: &str) -> Option<EngineSszEndpoint> {
    let mut segments = path.trim_start_matches('/').split('/');
    match (segments.next(), segments.next(), segments.next(), segments.next(), segments.next()) {
        (Some("engine"), Some("v1"), Some("capabilities"), None, None) => {
            Some(EngineSszEndpoint::Capabilities)
        }
        (Some("engine"), Some("v1"), Some("identity"), None, None) => {
            Some(EngineSszEndpoint::Identity)
        }
        (Some("engine"), Some("v1"), Some("payloads"), None, None) => {
            Some(EngineSszEndpoint::NewPayload)
        }
        (Some("engine"), Some("v1"), Some("payloads"), Some("witness"), None) => {
            Some(EngineSszEndpoint::PayloadsWithWitness)
        }
        (Some("engine"), Some("v1"), Some("payloads"), Some(payload_id), None) => {
            let payload_id = payload_id.parse::<PayloadId>();
            Some(EngineSszEndpoint::GetPayload(payload_id))
        }
        (Some("engine"), Some("v1"), Some("forkchoice"), None, None) => {
            Some(EngineSszEndpoint::Forkchoice)
        }
        (Some("engine"), Some("v1"), Some("blobs"), version, None) => {
            Some(EngineSszEndpoint::Blobs(parse_method_version(version?)?))
        }
        (Some("engine"), Some("v1"), Some("bodies"), Some("hash"), None) => {
            Some(EngineSszEndpoint::PayloadBodiesByHash)
        }
        (Some("engine"), Some("v1"), Some("bodies"), None, None) => {
            Some(EngineSszEndpoint::PayloadBodiesByRange)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EngineSszEndpoint {
    Capabilities,
    Identity,
    NewPayload,
    PayloadsWithWitness,
    GetPayload(Result<PayloadId, <PayloadId as std::str::FromStr>::Err>),
    Forkchoice,
    PayloadBodiesByHash,
    PayloadBodiesByRange,
    Blobs(u8),
}

/// Fork selector used by SSZ Engine API request handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineSszFork {
    /// Paris fork.
    Paris,
    /// Shanghai fork.
    Shanghai,
    /// Cancun fork.
    Cancun,
    /// Prague fork.
    Prague,
    /// Osaka fork.
    Osaka,
    /// Amsterdam fork.
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

fn handle_capabilities(witness_enabled: bool) -> HttpResponse {
    let mut fork_scoped_endpoints = vec!["payloads", "forkchoice", "bodies"];
    if witness_enabled {
        fork_scoped_endpoints.push("payloads/witness");
    }
    json_response(serde_json::json!({
        "supported_forks": ["paris", "shanghai", "cancun", "prague", "osaka", "amsterdam"],
        "fork_scoped_endpoints": fork_scoped_endpoints,
        "independently_versioned": {
            "blobs": ["v1", "v2", "v3", "v4"],
        },
        "unscoped_endpoints": ["capabilities", "identity"],
        "limits": {
            "bodies.max_count": MAX_BODIES_REQUEST,
            "blobs.max_versioned_hashes": MAX_BLOB_LIMIT,
            "payload.max_bytes": MAX_PAYLOAD_BYTES,
        },
    }))
}

fn handle_identity<Provider, Pool, Validator, ChainSpec>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    json_response(vec![engine_api.client_version().clone()])
}

async fn handle_get_payload<Provider, Pool, Validator, ChainSpec>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    fork: EngineSszFork,
    payload_id: PayloadId,
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    match fork {
        EngineSszFork::Paris => match engine_api.get_payload_v2_metered(payload_id).await {
            Ok(payload) => {
                let block_value = payload.block_value;
                match payload.execution_payload {
                    ExecutionPayloadFieldV2::V1(payload) => {
                        get_payload_response(BuiltPayloadParis { payload, block_value })
                    }
                    ExecutionPayloadFieldV2::V2(_) => {
                        problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
                    }
                }
            }
            Err(err) => engine_error_response(err),
        },
        EngineSszFork::Shanghai => match engine_api.get_payload_v2_metered(payload_id).await {
            Ok(payload) => match BuiltPayloadShanghai::try_from(payload) {
                Ok(payload) => get_payload_response(payload),
                Err(err) => problem_response(422, "invalid-body", Some(err.to_string())),
            },
            Err(err) => engine_error_response(err),
        },
        EngineSszFork::Cancun => match engine_api.get_payload_v3_metered(payload_id).await {
            Ok(payload) => get_payload_response(BuiltPayloadCancun::from(payload)),
            Err(err) => engine_error_response(err),
        },
        EngineSszFork::Prague => match engine_api.get_payload_v4_metered(payload_id).await {
            Ok(payload) => get_payload_response(BuiltPayloadPrague::from(payload)),
            Err(err) => engine_error_response(err),
        },
        EngineSszFork::Osaka => match engine_api.get_payload_v5_metered(payload_id).await {
            Ok(payload) => get_payload_response(BuiltPayloadOsaka::from(payload)),
            Err(err) => engine_error_response(err),
        },
        EngineSszFork::Amsterdam => match engine_api.get_payload_v6_metered(payload_id).await {
            Ok(payload) => get_payload_response(BuiltPayloadAmsterdam::from(payload)),
            Err(err) => engine_error_response(err),
        },
    }
}

fn engine_error_response(err: EngineApiError) -> HttpResponse {
    let detail = err.to_string();
    let error: jsonrpsee::types::ErrorObjectOwned = err.into();
    let (status, problem_type) = match error.code() {
        -32700 => (400, "parse-error"),
        -32600 => (400, "invalid-request"),
        -32601 => (404, "method-not-found"),
        -32602 => (422, "invalid-body"),
        -38001 => (404, "unknown-payload"),
        -38002 => (409, "invalid-forkchoice"),
        -38003 => (422, "invalid-attributes"),
        -38004 => (413, "request-too-large"),
        -38005 => (400, "unsupported-fork"),
        -38006 => (409, "reorg-too-deep"),
        _ => (500, "internal"),
    };
    problem_response(status, problem_type, Some(detail))
}

async fn handle_new_payload<Provider, Pool, Validator, ChainSpec>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    fork: EngineSszFork,
    body: &[u8],
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let payload = match decode_new_payload_request(fork, body) {
        Ok(payload) => payload,
        Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
    };

    let response = match fork.payloads_version() {
        1 => engine_api.new_payload_v1(payload).await,
        2 => engine_api.new_payload_v2(payload).await,
        3 => engine_api.new_payload_v3(payload).await,
        4 => engine_api.new_payload_v4(payload).await,
        5 => engine_api.new_payload_v5(payload).await,
        _ => return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None),
    };

    match response {
        Ok(status) => match EngineSszPayloadStatus::try_from(status) {
            Ok(status) => ssz_response(status),
            Err(err) => {
                problem_response(STATUS_INTERNAL_SERVER_ERROR, "internal", Some(err.to_string()))
            }
        },
        Err(err) => engine_error_response(err),
    }
}

async fn handle_new_payload_with_witness<Provider, Pool, Validator, ChainSpec>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    witness_handler: Arc<dyn EngineSszWitness>,
    body: &[u8],
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let payload = match decode_new_payload_request(EngineSszFork::Amsterdam, body) {
        Ok(payload) => payload,
        Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
    };

    let response = engine_api.new_payload_v5(payload.clone()).await;

    let status = match response {
        Ok(status) => match EngineSszPayloadStatus::try_from(status) {
            Ok(status) => status,
            Err(err) => {
                return problem_response(
                    STATUS_INTERNAL_SERVER_ERROR,
                    "internal",
                    Some(err.to_string()),
                )
            }
        },
        Err(err) => return engine_error_response(err),
    };

    let witness = if matches!(&status.status, alloy_rpc_types_engine::PayloadStatusEnum::Valid) {
        match witness_handler.generate_witness(payload).await {
            Ok(witness) => Some(witness),
            Err(err) => {
                return problem_response(STATUS_INTERNAL_SERVER_ERROR, "internal", Some(err))
            }
        }
    } else {
        None
    };

    ssz_response(PayloadStatusWithWitness::new(status, witness))
}

async fn handle_forkchoice_updated<Provider, Pool, Validator, ChainSpec>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    fork: EngineSszFork,
    body: &[u8],
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let (state, attrs, custody_columns) = match decode_forkchoice_request(fork, body) {
        Ok(request) => request,
        Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
    };

    let response = match fork.forkchoice_version() {
        1 => engine_api.fork_choice_updated_v1_metered(state, attrs).await,
        2 => engine_api.fork_choice_updated_v2_metered(state, attrs).await,
        3 => engine_api.fork_choice_updated_v3_metered(state, attrs).await,
        4 => engine_api.fork_choice_updated_v4_metered(state, attrs, custody_columns).await,
        _ => return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None),
    };

    match response {
        Ok(updated) => match ForkchoiceUpdateResponse::try_from(updated) {
            Ok(updated) => ssz_response(updated),
            Err(err) => {
                problem_response(STATUS_INTERNAL_SERVER_ERROR, "internal", Some(err.to_string()))
            }
        },
        Err(err) => engine_error_response(err),
    }
}

enum PayloadBodiesRequest {
    Hash(Vec<B256>),
    Range { start: u64, count: u64 },
}

async fn handle_get_payload_bodies<Provider, Pool, Validator, ChainSpec>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    fork: EngineSszFork,
    request: PayloadBodiesRequest,
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let include_bal = fork == EngineSszFork::Amsterdam;
    let response = match request {
        PayloadBodiesRequest::Hash(hashes) => {
            engine_api.get_payload_bodies_by_hash_with_timestamps(hashes, include_bal).await
        }
        PayloadBodiesRequest::Range { start, count } => {
            engine_api.get_payload_bodies_by_range_with_timestamps(start, count, include_bal).await
        }
    };
    let chain_spec = engine_api.chain_spec();
    if include_bal {
        return payload_bodies_http_response(
            response,
            |body| ExecutionPayloadBodyAmsterdam::try_from(body).ok(),
            fork,
            chain_spec.as_ref(),
        )
    }
    let response = response.map(|bodies| {
        bodies
            .into_iter()
            .map(|body| {
                body.map(|(timestamp, body)| {
                    (
                        timestamp,
                        ExecutionPayloadBodyV1 {
                            transactions: body.transactions,
                            withdrawals: body.withdrawals,
                        },
                    )
                })
            })
            .collect()
    });
    if fork == EngineSszFork::Paris {
        payload_bodies_http_response(
            response,
            |body| ExecutionPayloadBodyParis::try_from(body).ok(),
            fork,
            chain_spec.as_ref(),
        )
    } else {
        payload_bodies_http_response(
            response,
            |body| ExecutionPayloadBodyShanghai::try_from(body).ok(),
            fork,
            chain_spec.as_ref(),
        )
    }
}

fn payload_bodies_response<LegacyBody, ForkBody>(
    response: Result<Vec<Option<(u64, LegacyBody)>>, EngineApiError>,
    convert: impl Fn(LegacyBody) -> Option<ForkBody>,
    fork: EngineSszFork,
    chain_spec: &impl EthereumHardforks,
) -> Result<BodiesResponse<ForkBody>, EngineApiError>
where
    ForkBody: Default + ssz::Encode + ssz::Decode,
{
    let bodies = response?;
    let bodies = bodies
        .into_iter()
        .map(|body| {
            body.filter(|(timestamp, _)| body_matches_fork(chain_spec, fork, *timestamp))
                .map(|(_, body)| body)
        })
        .collect();
    Ok(BodiesResponse::from_optional_bodies(bodies, convert))
}

fn payload_bodies_http_response<LegacyBody, ForkBody>(
    response: Result<Vec<Option<(u64, LegacyBody)>>, EngineApiError>,
    convert: impl Fn(LegacyBody) -> Option<ForkBody>,
    fork: EngineSszFork,
    chain_spec: &impl EthereumHardforks,
) -> HttpResponse
where
    ForkBody: Default + ssz::Encode + ssz::Decode,
{
    match payload_bodies_response(response, convert, fork, chain_spec) {
        Ok(response) => ssz_response(response),
        Err(err) => engine_error_response(err),
    }
}

fn body_matches_fork<ChainSpec: EthereumHardforks>(
    chain_spec: &ChainSpec,
    fork: EngineSszFork,
    timestamp: u64,
) -> bool {
    let active = |fork| chain_spec.is_ethereum_fork_active_at_timestamp(fork, timestamp);
    match fork {
        EngineSszFork::Paris => !active(EthereumHardfork::Shanghai),
        EngineSszFork::Shanghai => {
            active(EthereumHardfork::Shanghai) && !active(EthereumHardfork::Cancun)
        }
        EngineSszFork::Cancun => {
            active(EthereumHardfork::Cancun) && !active(EthereumHardfork::Prague)
        }
        EngineSszFork::Prague => {
            active(EthereumHardfork::Prague) && !active(EthereumHardfork::Osaka)
        }
        EngineSszFork::Osaka => {
            active(EthereumHardfork::Osaka) && !active(EthereumHardfork::Amsterdam)
        }
        EngineSszFork::Amsterdam => active(EthereumHardfork::Amsterdam),
    }
}

async fn read_ssz_body(request: HttpRequest, max_bytes: u64) -> Result<Bytes, HttpResponse> {
    let content_type = request.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok());
    if content_type != Some(OCTET_STREAM) {
        return Err(problem_response(STATUS_UNSUPPORTED_MEDIA_TYPE, "unsupported-media-type", None))
    }

    if let Some(content_length) = request.headers().get("content-length") {
        let Some(content_length) =
            content_length.to_str().ok().and_then(|value| value.parse::<u64>().ok())
        else {
            return Err(problem_response(STATUS_BAD_REQUEST, "invalid-request", None))
        };
        if content_length > max_bytes {
            return Err(problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None))
        }
    }

    let Ok(limit) = usize::try_from(max_bytes) else {
        return Err(problem_response(STATUS_INTERNAL_SERVER_ERROR, "internal", None))
    };
    match Limited::new(request.into_body(), limit).collect().await {
        Ok(body) => Ok(body.to_bytes().into()),
        Err(err) if err.downcast_ref::<LengthLimitError>().is_some() => {
            Err(problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None))
        }
        Err(_) => Err(problem_response(STATUS_BAD_REQUEST, "invalid-request", None)),
    }
}

/// Handles SSZ `engine_getBlobsV*` requests with the node's blob store.
async fn handle_get_blobs<ChainSpec, Provider, Pool, Validator>(
    engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    version: u8,
    body: &[u8],
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let request = if version == 4 {
        BlobsV4Request::from_ssz_bytes(body)
    } else {
        BlobsV1Request::from_ssz_bytes(body).map(|request| BlobsV4Request {
            versioned_hashes: request.versioned_hashes,
            indices_bitarray: B128::ZERO,
        })
    };
    let request = match request {
        Ok(request) => request,
        Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
    };
    if request.versioned_hashes.len() > MAX_BLOB_LIMIT {
        return problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None)
    }
    let hashes = request.versioned_hashes;
    match version {
        1 => blob_response::<BlobsV1Response, _>(engine_api.get_blobs_v1_metered(hashes).map(Some)),
        2 => blob_response::<BlobsV2Response, _>(engine_api.get_blobs_v2_metered(hashes)),
        3 => blob_response::<BlobsV3Response, _>(engine_api.get_blobs_v3_metered(hashes)),
        4 => blob_response::<BlobsV4Response, _>(
            engine_api.get_blobs_v4_metered(hashes, request.indices_bitarray),
        ),
        _ => problem_response(STATUS_NOT_FOUND, "method-not-found", None),
    }
}

fn blob_response<Ssz, Legacy>(response: Result<Option<Legacy>, EngineApiError>) -> HttpResponse
where
    Ssz: TryFrom<Legacy> + ssz::Encode,
    Ssz::Error: std::fmt::Display,
{
    match response {
        Ok(Some(response)) => match Ssz::try_from(response) {
            Ok(response) => ssz_response(response),
            Err(err) => {
                problem_response(STATUS_INTERNAL_SERVER_ERROR, "internal", Some(err.to_string()))
            }
        },
        Ok(None) => no_content_response(),
        Err(err) => engine_error_response(err),
    }
}

/// API surface required by the SSZ Engine API proxy.
pub trait EngineSszApi: Clone + Send + Sync + 'static {
    /// Whether the implementation supports the witness extension.
    fn supports_witness(&self) -> bool {
        false
    }

    /// Returns the Engine API identity response.
    fn identity(&self) -> HttpResponse;

    /// Handles a new payload request.
    fn new_payload(
        &self,
        fork: EngineSszFork,
        body: Bytes,
    ) -> impl Future<Output = HttpResponse> + Send;

    /// Handles the experimental Amsterdam payload submission with a witness.
    fn new_payload_with_witness(
        &self,
        _body: Bytes,
        _witness_handler: Arc<dyn EngineSszWitness>,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Handles a getPayload request.
    fn get_payload(
        &self,
        fork: EngineSszFork,
        payload_id: PayloadId,
    ) -> impl Future<Output = HttpResponse> + Send;

    /// Handles a forkchoice update request.
    fn forkchoice_updated(
        &self,
        fork: EngineSszFork,
        body: Bytes,
    ) -> impl Future<Output = HttpResponse> + Send;

    /// Handles a getPayloadBodiesByHash request.
    fn get_payload_bodies_by_hash(
        &self,
        fork: EngineSszFork,
        hashes: Vec<B256>,
    ) -> impl Future<Output = HttpResponse> + Send;

    /// Handles a getPayloadBodiesByRange request.
    fn get_payload_bodies_by_range(
        &self,
        fork: EngineSszFork,
        start: u64,
        count: u64,
    ) -> impl Future<Output = HttpResponse> + Send;

    /// Handles a getBlobs request.
    fn get_blobs(&self, version: u8, body: Bytes) -> impl Future<Output = HttpResponse> + Send;
}

impl<Provider, Pool, Validator, ChainSpec> EngineSszApi
    for EthEngineApi<Provider, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    fn supports_witness(&self) -> bool {
        true
    }

    async fn new_payload_with_witness(
        &self,
        body: Bytes,
        witness_handler: Arc<dyn EngineSszWitness>,
    ) -> HttpResponse {
        handle_new_payload_with_witness(self.clone(), witness_handler, &body).await
    }

    fn identity(&self) -> HttpResponse {
        handle_identity(self.clone())
    }

    fn new_payload(
        &self,
        fork: EngineSszFork,
        body: Bytes,
    ) -> impl Future<Output = HttpResponse> + Send {
        let engine_api = self.clone();
        async move { handle_new_payload(engine_api, fork, body.as_ref()).await }
    }

    fn get_payload(
        &self,
        fork: EngineSszFork,
        payload_id: PayloadId,
    ) -> impl Future<Output = HttpResponse> + Send {
        let engine_api = self.clone();
        async move { handle_get_payload(engine_api, fork, payload_id).await }
    }

    fn forkchoice_updated(
        &self,
        fork: EngineSszFork,
        body: Bytes,
    ) -> impl Future<Output = HttpResponse> + Send {
        let engine_api = self.clone();
        async move { handle_forkchoice_updated(engine_api, fork, body.as_ref()).await }
    }

    async fn get_payload_bodies_by_hash(
        &self,
        fork: EngineSszFork,
        hashes: Vec<B256>,
    ) -> HttpResponse {
        handle_get_payload_bodies(self.clone(), fork, PayloadBodiesRequest::Hash(hashes)).await
    }

    async fn get_payload_bodies_by_range(
        &self,
        fork: EngineSszFork,
        start: u64,
        count: u64,
    ) -> HttpResponse {
        handle_get_payload_bodies(self.clone(), fork, PayloadBodiesRequest::Range { start, count })
            .await
    }

    fn get_blobs(&self, version: u8, body: Bytes) -> impl Future<Output = HttpResponse> + Send {
        let engine_api = self.clone();
        async move { handle_get_blobs(engine_api, version, body.as_ref()).await }
    }
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
) -> Result<(ForkchoiceState, Option<PayloadAttributes>, Option<B128>), &'static str> {
    match fork {
        EngineSszFork::Paris => {
            let ForkchoiceUpdateParis { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateParis::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, optional_attrs(payload_attributes), None))
        }
        EngineSszFork::Shanghai => {
            let ForkchoiceUpdateShanghai { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateShanghai::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, optional_attrs(payload_attributes), None))
        }
        EngineSszFork::Cancun => {
            let ForkchoiceUpdateCancun { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateCancun::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, optional_attrs(payload_attributes), None))
        }
        EngineSszFork::Prague => {
            let ForkchoiceUpdatePrague { forkchoice_state, payload_attributes } =
                ForkchoiceUpdatePrague::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, optional_attrs(payload_attributes), None))
        }
        EngineSszFork::Osaka => {
            let ForkchoiceUpdateOsaka { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateOsaka::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, optional_attrs(payload_attributes), None))
        }
        EngineSszFork::Amsterdam => {
            let ForkchoiceUpdateAmsterdam { forkchoice_state, payload_attributes, custody_columns } =
                ForkchoiceUpdateAmsterdam::from_ssz_bytes(body).map_err(|_| "invalid ssz")?;
            Ok((
                forkchoice_state,
                optional_attrs(payload_attributes),
                custody_columns.into_option(),
            ))
        }
    }
}

fn optional_attrs<T>(attrs: Optional<T>) -> Option<PayloadAttributes>
where
    T: Into<PayloadAttributes>,
{
    attrs.into_option().map(Into::into)
}

fn ssz_response<T: ssz::Encode>(value: T) -> HttpResponse {
    HttpResponse::builder()
        .status(STATUS_OK)
        .header(CONTENT_TYPE, OCTET_STREAM)
        .body(HttpBody::from(value.as_ssz_bytes()))
        .expect("valid response")
}

fn get_payload_response<T: ssz::Encode>(value: T) -> HttpResponse {
    HttpResponse::builder()
        .status(STATUS_OK)
        .header(CONTENT_TYPE, OCTET_STREAM)
        .header(CACHE_CONTROL, "no-store")
        .body(HttpBody::from(value.as_ssz_bytes()))
        .expect("valid response")
}

fn json_response<T: serde::Serialize>(value: T) -> HttpResponse {
    let Ok(body) = serde_json::to_string(&value) else {
        return problem_response(
            STATUS_INTERNAL_SERVER_ERROR,
            "internal",
            Some("failed to encode json".to_string()),
        )
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

fn problem_response(
    status: u16,
    problem_type: &'static str,
    detail: Option<String>,
) -> HttpResponse {
    let problem_type = format!("/engine-api/errors/{problem_type}");
    let body = match detail {
        Some(detail) => serde_json::json!({ "type": problem_type, "detail": detail }),
        None => serde_json::json!({ "type": problem_type }),
    };

    HttpResponse::builder()
        .status(status)
        .header(CONTENT_TYPE, PROBLEM_JSON)
        .body(HttpBody::from(body.to_string()))
        .expect("valid response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::Encode;

    #[tokio::test]
    async fn witness_is_only_advertised_when_configured() {
        for enabled in [false, true] {
            let body = handle_capabilities(enabled).into_body().collect().await.unwrap().to_bytes();
            let capabilities: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let endpoints = capabilities["fork_scoped_endpoints"].as_array().unwrap();
            assert_eq!(endpoints.iter().any(|endpoint| endpoint == "payloads/witness"), enabled);
        }
        assert_eq!(
            parse_engine_path("/engine/v1/payloads/witness"),
            Some(EngineSszEndpoint::PayloadsWithWitness)
        );
    }

    #[tokio::test]
    async fn post_body_limits_apply_without_content_length() {
        for size in [16, 17] {
            let request = HttpRequest::builder()
                .header(CONTENT_TYPE, OCTET_STREAM)
                .body(HttpBody::from(vec![0; size]))
                .unwrap();
            let response = read_ssz_body(request, 16).await;
            if size == 16 {
                assert_eq!(response.unwrap().len(), size);
            } else {
                assert_eq!(response.unwrap_err().status(), STATUS_PAYLOAD_TOO_LARGE);
            }
        }
    }

    #[tokio::test]
    async fn engine_errors_preserve_validation_semantics() {
        use alloy_rpc_types_engine::ForkchoiceUpdateError;
        use reth_engine_primitives::BeaconForkChoiceUpdateError;
        for (error, status, kind) in [
            (EngineApiError::UnknownPayload, 404, "unknown-payload"),
            (EngineApiError::BlobRequestTooLarge { len: 129 }, 413, "request-too-large"),
            (
                EngineApiError::ForkChoiceUpdate(
                    BeaconForkChoiceUpdateError::ForkchoiceUpdateError(
                        ForkchoiceUpdateError::InvalidState,
                    ),
                ),
                409,
                "invalid-forkchoice",
            ),
            (
                EngineApiError::ForkChoiceUpdate(
                    BeaconForkChoiceUpdateError::ForkchoiceUpdateError(
                        ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes,
                    ),
                ),
                422,
                "invalid-attributes",
            ),
        ] {
            let response = engine_error_response(error);
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(problem["type"], format!("/engine-api/errors/{kind}"));
        }
    }

    #[test]
    fn payload_bodies_are_filtered_at_fork_boundaries() {
        use reth_chainspec::{ChainSpecBuilder, ForkCondition};
        let chain_spec = ChainSpecBuilder::default()
            .chain(1.into())
            .genesis(Default::default())
            .with_fork(EthereumHardfork::Shanghai, ForkCondition::Timestamp(10))
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(20))
            .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(30))
            .with_fork(EthereumHardfork::Osaka, ForkCondition::Timestamp(40))
            .with_fork(EthereumHardfork::Amsterdam, ForkCondition::Timestamp(50))
            .build();
        for (fork, start, end) in [
            (EngineSszFork::Shanghai, 10, 20),
            (EngineSszFork::Cancun, 20, 30),
            (EngineSszFork::Prague, 30, 40),
            (EngineSszFork::Osaka, 40, 50),
        ] {
            let body = ExecutionPayloadBodyV1 {
                transactions: vec![Bytes::from_static(&[1, 2, 3])],
                withdrawals: Some(vec![]),
            };
            let response = payload_bodies_response(
                Ok(vec![
                    Some((start - 1, body.clone())),
                    Some((start, body.clone())),
                    Some((end - 1, body.clone())),
                    Some((end, body)),
                    None,
                ]),
                |body| ExecutionPayloadBodyShanghai::try_from(body).ok(),
                fork,
                &chain_spec,
            )
            .unwrap();
            assert_eq!(
                response.entries.iter().map(|entry| entry.available).collect::<Vec<_>>(),
                [false, true, true, false, false]
            );
            for index in [0, 3, 4] {
                assert_eq!(response.entries[index].body, ExecutionPayloadBodyShanghai::default());
            }
        }
        assert!(body_matches_fork(&chain_spec, EngineSszFork::Paris, 9));
        assert!(!body_matches_fork(&chain_spec, EngineSszFork::Paris, 10));
        assert!(!body_matches_fork(&chain_spec, EngineSszFork::Amsterdam, 49));
        assert!(body_matches_fork(&chain_spec, EngineSszFork::Amsterdam, 50));
    }

    #[tokio::test]
    async fn payload_bodies_hash_request_limits_and_media_type() {
        for content_length in [false, true] {
            for count in [32, 33] {
                let bytes =
                    BodiesByHashRequest { block_hashes: vec![B256::ZERO; count] }.as_ssz_bytes();
                let mut request = HttpRequest::builder().header(CONTENT_TYPE, OCTET_STREAM);
                if content_length {
                    request = request.header("content-length", bytes.len());
                }
                let result = read_ssz_body(
                    request.body(HttpBody::from(bytes)).unwrap(),
                    MAX_BODIES_REQUEST_BYTES,
                )
                .await;
                if count == 32 {
                    assert_eq!(
                        BodiesByHashRequest::from_ssz_bytes(&result.unwrap())
                            .unwrap()
                            .block_hashes
                            .len(),
                        32
                    );
                } else {
                    let response = result.unwrap_err();
                    assert_eq!(response.status(), STATUS_PAYLOAD_TOO_LARGE);
                    assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
                }
            }
        }
        let response = read_ssz_body(HttpRequest::new(HttpBody::empty()), MAX_BODIES_REQUEST_BYTES)
            .await
            .unwrap_err();
        assert_eq!(response.status(), STATUS_UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn payload_bodies_errors_and_capabilities_match_rest_spec() {
        let response =
            engine_error_response(EngineApiError::InvalidBodiesRange { start: 0, count: 1 });
        assert_eq!(response.status(), 422);
        assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["type"], "/engine-api/errors/invalid-body");
        let body = handle_capabilities(false).into_body().collect().await.unwrap().to_bytes();
        let capabilities: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(capabilities["limits"]["bodies.max_count"], 32);
    }

    #[test]
    fn parses_capabilities_endpoint() {
        let endpoint = parse_engine_path("/engine/v1/capabilities").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Capabilities);
    }

    #[test]
    fn parses_identity_endpoint() {
        let endpoint = parse_engine_path("/engine/v1/identity").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Identity);
    }

    #[test]
    fn parses_fork_scoped_payload_endpoint() {
        let endpoint = parse_engine_path("/engine/v1/payloads").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::NewPayload);
    }

    #[test]
    fn parses_fork_scoped_get_payload_endpoint() {
        let endpoint = parse_engine_path("/engine/v1/payloads/0x0000000000000001").unwrap();
        assert_eq!(
            endpoint,
            EngineSszEndpoint::GetPayload(Ok(PayloadId::new([0, 0, 0, 0, 0, 0, 0, 1])))
        );
    }

    #[test]
    fn matches_malformed_get_payload_endpoint() {
        assert!(matches!(
            parse_engine_path("/engine/v1/payloads/0x01"),
            Some(EngineSszEndpoint::GetPayload(Err(_)))
        ));
    }

    #[test]
    fn parses_fork_scoped_forkchoice_endpoint() {
        let endpoint = parse_engine_path("/engine/v1/forkchoice").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Forkchoice);
    }

    #[test]
    fn parses_payload_bodies_endpoints() {
        assert_eq!(
            parse_engine_path("/engine/v1/bodies/hash").unwrap(),
            EngineSszEndpoint::PayloadBodiesByHash
        );
        assert_eq!(
            parse_engine_path("/engine/v1/bodies").unwrap(),
            EngineSszEndpoint::PayloadBodiesByRange
        );
    }

    #[test]
    fn rejects_legacy_version_scoped_endpoint() {
        assert!(parse_engine_path("/engine/v4/payloads").is_none());
    }

    #[test]
    fn decodes_blob_request_container() {
        let hashes = vec![B256::ZERO, B256::with_last_byte(1)];
        let decoded = BlobsV1Request::from_ssz_bytes(
            &BlobsV1Request { versioned_hashes: hashes.clone() }.as_ssz_bytes(),
        )
        .unwrap()
        .versioned_hashes;
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

        let (decoded_state, decoded_attrs, custody_columns) =
            decode_forkchoice_request(EngineSszFork::Amsterdam, &encoded).unwrap();
        assert_eq!(decoded_state, forkchoice_state);
        assert!(decoded_attrs.is_none());
        assert_eq!(custody_columns, Some(B128::with_last_byte(1)));
    }

    #[test]
    fn decodes_forkchoice_cancun_payload_attributes() {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: B256::ZERO,
            safe_block_hash: B256::ZERO,
            finalized_block_hash: B256::ZERO,
        };
        let attrs = crate::engine_ssz_containers::PayloadAttributesCancun {
            timestamp: 1,
            prev_randao: B256::with_last_byte(2),
            suggested_fee_recipient: Default::default(),
            withdrawals: Vec::new(),
            parent_beacon_block_root: B256::with_last_byte(3),
        };
        let encoded =
            ForkchoiceUpdateCancun { forkchoice_state, payload_attributes: Optional::some(attrs) }
                .as_ssz_bytes();

        let (decoded_state, decoded_attrs, custody_columns) =
            decode_forkchoice_request(EngineSszFork::Cancun, &encoded).unwrap();
        assert_eq!(decoded_state, forkchoice_state);
        let decoded_attrs = decoded_attrs.unwrap();
        assert_eq!(decoded_attrs.timestamp, 1);
        assert!(decoded_attrs.withdrawals.as_ref().unwrap().is_empty());
        assert_eq!(decoded_attrs.parent_beacon_block_root, Some(B256::with_last_byte(3)));
        assert!(custody_columns.is_none());
    }
}
