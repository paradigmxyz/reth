//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v1`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

use crate::{
    engine_ssz_containers::{
        BlobsV1Request, BlobsV1Response, BlobsV2Response, BlobsV3Response, BlobsV4Request,
        BlobsV4Response, BodiesByHashRequest, BodiesResponse, BuiltPayloadAmsterdam,
        BuiltPayloadOsaka, BuiltPayloadParis, BuiltPayloadPrague, BuiltPayloadShanghai,
        ExecutionPayloadBodyAmsterdam, ExecutionPayloadBodyParis, ExecutionPayloadBodyShanghai,
        ExecutionPayloadEnvelopeAmsterdam, ExecutionPayloadEnvelopeCancun,
        ExecutionPayloadEnvelopeOsaka, ExecutionPayloadEnvelopeParis,
        ExecutionPayloadEnvelopePrague, ExecutionPayloadEnvelopeShanghai,
        ForkchoiceUpdateAmsterdam, ForkchoiceUpdateCancun, ForkchoiceUpdateOsaka,
        ForkchoiceUpdateParis, ForkchoiceUpdatePrague, ForkchoiceUpdateResponse,
        ForkchoiceUpdateShanghai, Optional, PayloadStatus as EngineSszPayloadStatus,
        PayloadStatusWithWitness,
    },
    engine_ssz_witness::{EngineSszWitness, EngineSszWitnessError},
};
use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{eip2718::Decodable2718, eip7685::Requests};
use alloy_primitives::{Bytes, B128, B256};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadBodyV1,
    ExecutionPayloadFieldV2, ExecutionPayloadSidecar, ForkchoiceState, PayloadAttributes,
    PayloadId, PayloadStatusEnum, PraguePayloadFields,
};
use futures::future::{BoxFuture, Either};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_chainspec::{EthereumHardfork, EthereumHardforks};
use reth_engine_primitives::EngineApiValidator;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_provider::{BalProvider, BlockReader, HeaderProvider, StateProviderFactory};
use reth_rpc::EngineApi;
use reth_rpc_engine_api::EngineApiError;
use reth_tracing::tracing::debug;
use reth_transaction_pool::TransactionPool;
use ssz::Decode;
use std::{
    future::Future,
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
const STATUS_CONFLICT: u16 = 409;
const STATUS_UNPROCESSABLE_ENTITY: u16 = 422;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
const STATUS_PAYLOAD_TOO_LARGE: u16 = 413;
const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_SERVICE_UNAVAILABLE: u16 = 503;
const STATUS_UNSUPPORTED_MEDIA_TYPE: u16 = 415;

const MAX_BLOB_LIMIT: usize = crate::engine_ssz_containers::MAX_BLOBS_REQUEST;
const MAX_BLOB_REQUEST_BYTES: usize = 4 + 16 + MAX_BLOB_LIMIT * 32;
const MAX_BODIES_REQUEST: usize = crate::engine_ssz_containers::MAX_BODIES_REQUEST;
const MAX_BODIES_REQUEST_BYTES: usize = 4 + MAX_BODIES_REQUEST * 32;
const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
const PROBLEM_JSON: &str = "application/problem+json";

type EthEngineApi<Provider, Pool, Validator, ChainSpec> =
    EngineApi<Provider, EthEngineTypes, Pool, Validator, ChainSpec>;
/// Shared handle used by [`EngineSszProxyLayer`].
pub struct EngineSszProxyHandle<Api = ()> {
    state: Arc<RwLock<EngineSszState<Api>>>,
}

impl<Api> Clone for EngineSszProxyHandle<Api> {
    fn clone(&self) -> Self {
        Self { state: self.state.clone() }
    }
}

impl<Api> std::fmt::Debug for EngineSszProxyHandle<Api> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSszProxyHandle").finish_non_exhaustive()
    }
}

impl<Api> EngineSszProxyHandle<Api> {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(EngineSszState {
                engine_api: None,
                witness_handler: None,
                witness_enabled: false,
            })),
        }
    }

    fn with_engine_api(engine_api: Api) -> Self {
        Self {
            state: Arc::new(RwLock::new(EngineSszState {
                engine_api: Some(engine_api),
                witness_handler: None,
                witness_enabled: false,
            })),
        }
    }

    /// Returns whether both the API and generator support the witness extension.
    pub async fn witness_enabled(&self) -> bool {
        self.state.read().await.witness_enabled
    }

    /// Returns the configured witness generator.
    pub async fn witness_handler(&self) -> Option<Arc<dyn EngineSszWitness>> {
        self.state.read().await.witness_handler.clone()
    }
}

impl<Api: EngineSszApi> EngineSszProxyHandle<Api> {
    /// Sets the Engine API implementation used by the proxy.
    pub async fn set_engine_api(&self, engine_api: Api) {
        let mut state = self.state.write().await;
        state.engine_api = Some(engine_api);
        state.update_witness_support();
    }

    /// Sets the Engine API implementation during synchronous launch wiring.
    pub fn set_engine_api_sync(&self, engine_api: Api) {
        let mut state =
            self.state.try_write().expect("engine api handle should not be locked during launch");
        state.engine_api = Some(engine_api);
        state.update_witness_support();
    }

    /// Sets the witness generator used by `/payloads/witness`.
    pub async fn set_witness_handler(&self, witness_handler: Arc<dyn EngineSszWitness>) {
        let mut state = self.state.write().await;
        state.witness_handler = Some(witness_handler);
        state.update_witness_support();
    }

    /// Sets the witness generator during synchronous launch wiring.
    pub fn set_witness_handler_sync(&self, witness_handler: Arc<dyn EngineSszWitness>) {
        let mut state =
            self.state.try_write().expect("witness handle should not be locked during launch");
        state.witness_handler = Some(witness_handler);
        state.update_witness_support();
    }
}

impl<Api: Clone> EngineSszProxyHandle<Api> {
    /// Returns the Engine API implementation used by the proxy.
    pub async fn engine_api(&self) -> Option<Api> {
        self.state.read().await.engine_api.clone()
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
    S::Future: Send,
    Api: EngineSszApi,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Either<S::Future, BoxFuture<'static, Result<HttpResponse, BoxError>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: HttpRequest) -> Self::Future {
        if !request.uri().path().starts_with("/engine/") {
            return Either::Left(self.inner.call(request))
        }

        let handle = self.handle.clone();
        Either::Right(Box::pin(async move { Ok(handle_engine_ssz_request(handle, request).await) }))
    }
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

/// API surface required by the SSZ Engine API proxy.
///
/// Custom APIs can opt in with a marker implementation; every route, including
/// `/capabilities`, then answers 404 so clients fall back to the JSON-RPC Engine API.
pub trait EngineSszApi: Clone + Send + Sync + 'static {
    /// Returns the capabilities advertisement.
    ///
    /// `witness_enabled` is true when a witness generator is configured and
    /// [`Self::supports_witness`] holds, so the advertisement can include the extension.
    fn capabilities(&self, _witness_enabled: bool) -> HttpResponse {
        problem_response(STATUS_NOT_FOUND, "method-not-found", None)
    }

    /// Whether the implementation supports the witness extension.
    fn supports_witness(&self) -> bool {
        false
    }

    /// Handles the experimental Amsterdam payload submission with a witness.
    fn new_payload_with_witness(
        &self,
        _body: Bytes,
        _witness_handler: Arc<dyn EngineSszWitness>,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Returns the Engine API identity response.
    fn identity(&self) -> HttpResponse {
        problem_response(STATUS_NOT_FOUND, "method-not-found", None)
    }

    /// Handles a new payload request.
    fn new_payload(
        &self,
        _fork: EngineSszFork,
        _body: Bytes,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Handles a getPayload request.
    fn get_payload(
        &self,
        _fork: EngineSszFork,
        _payload_id: PayloadId,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Handles a forkchoice update request.
    fn forkchoice_updated(
        &self,
        _fork: EngineSszFork,
        _body: Bytes,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Handles a getPayloadBodiesByHash request.
    fn get_payload_bodies_by_hash(
        &self,
        _fork: EngineSszFork,
        _hashes: Vec<B256>,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Handles a getPayloadBodiesByRange request.
    fn get_payload_bodies_by_range(
        &self,
        _fork: EngineSszFork,
        _start: u64,
        _count: u64,
    ) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }

    /// Handles a getBlobs request.
    fn get_blobs(&self, _version: u8, _body: Bytes) -> impl Future<Output = HttpResponse> + Send {
        async { problem_response(STATUS_NOT_FOUND, "method-not-found", None) }
    }
}

impl EngineSszApi for reth_node_builder::rpc::NoopEngineApi {}

impl<Provider, Pool, Validator, ChainSpec> EngineSszApi
    for EthEngineApi<Provider, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    fn capabilities(&self, witness_enabled: bool) -> HttpResponse {
        handle_capabilities(witness_enabled)
    }

    fn identity(&self) -> HttpResponse {
        json_response(vec![self.client_version().clone()])
    }

    async fn new_payload(&self, fork: EngineSszFork, body: Bytes) -> HttpResponse {
        let payload = match decode_new_payload_request(fork, &body) {
            Ok(payload) => payload,
            Err(err) => return err.into_response(),
        };

        match submit_payload(self, fork, payload).await {
            Ok(status) => ssz_response(status),
            Err(response) => response,
        }
    }

    fn supports_witness(&self) -> bool {
        true
    }

    async fn new_payload_with_witness(
        &self,
        body: Bytes,
        witness_handler: Arc<dyn EngineSszWitness>,
    ) -> HttpResponse {
        let payload = match decode_new_payload_request(EngineSszFork::Amsterdam, &body) {
            Ok(payload) => payload,
            Err(err) => return err.into_response(),
        };
        let status = match submit_payload(self, EngineSszFork::Amsterdam, payload.clone()).await {
            Ok(status) => status,
            Err(response) => return response,
        };
        let witness = match status.status {
            PayloadStatusEnum::Valid => match witness_handler.generate_witness(payload).await {
                Ok(witness) => Some(witness),
                // The block is valid but its parent is only known to the engine tree. The
                // status stays authoritative; resubmitting once forkchoice has made the parent
                // canonical yields the witness.
                Err(EngineSszWitnessError::ParentStateUnavailable { parent, source }) => {
                    debug!(
                        target: "engine::ssz",
                        %parent,
                        %source,
                        "witness omitted for valid payload"
                    );
                    None
                }
                Err(err) => {
                    return problem_response(
                        STATUS_INTERNAL_SERVER_ERROR,
                        "internal",
                        Some(err.to_string()),
                    )
                }
            },
            _ => None,
        };
        ssz_response(PayloadStatusWithWitness::new(status, witness))
    }

    async fn get_payload(&self, fork: EngineSszFork, payload_id: PayloadId) -> HttpResponse {
        match fork {
            EngineSszFork::Paris => match self.get_payload_v2_metered(payload_id).await {
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
            EngineSszFork::Shanghai => match self.get_payload_v2_metered(payload_id).await {
                Ok(payload) => match BuiltPayloadShanghai::try_from(payload) {
                    Ok(payload) => get_payload_response(payload),
                    Err(err) => problem_response(
                        STATUS_UNPROCESSABLE_ENTITY,
                        "invalid-body",
                        Some(err.to_string()),
                    ),
                },
                Err(err) => engine_error_response(err),
            },
            EngineSszFork::Cancun => self
                .get_payload_v3_metered(payload_id)
                .await
                .map_or_else(engine_error_response, get_payload_response),
            EngineSszFork::Prague => self
                .get_payload_v4_metered(payload_id)
                .await
                .map(BuiltPayloadPrague::from)
                .map_or_else(engine_error_response, get_payload_response),
            EngineSszFork::Osaka => self
                .get_payload_v5_metered(payload_id)
                .await
                .map(BuiltPayloadOsaka::from)
                .map_or_else(engine_error_response, get_payload_response),
            EngineSszFork::Amsterdam => self
                .get_payload_v6_metered(payload_id)
                .await
                .map(BuiltPayloadAmsterdam::from)
                .map_or_else(engine_error_response, get_payload_response),
        }
    }

    async fn forkchoice_updated(&self, fork: EngineSszFork, body: Bytes) -> HttpResponse {
        let (state, attrs, custody_columns) = match decode_forkchoice_request(fork, &body) {
            Ok(request) => request,
            Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
        };

        if attrs.as_ref().is_some_and(|attrs| {
            !timestamp_matches_fork(self.chain_spec().as_ref(), fork, attrs.timestamp)
        }) {
            return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
        }

        let response = match fork.forkchoice_version() {
            1 => self.fork_choice_updated_v1_metered(state, attrs).await,
            2 => self.fork_choice_updated_v2_metered(state, attrs).await,
            3 => self.fork_choice_updated_v3_metered(state, attrs).await,
            4 => self.fork_choice_updated_v4_metered(state, attrs, custody_columns).await,
            _ => return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None),
        };

        match response {
            Ok(updated) => match ForkchoiceUpdateResponse::try_from(updated) {
                Ok(updated) => ssz_response(updated),
                Err(err) => problem_response(
                    STATUS_INTERNAL_SERVER_ERROR,
                    "internal",
                    Some(err.to_string()),
                ),
            },
            Err(err) => engine_error_response(err),
        }
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

    async fn get_blobs(&self, version: u8, body: Bytes) -> HttpResponse {
        if version == 4 {
            let request = match BlobsV4Request::from_ssz_bytes(&body) {
                Ok(request) => request,
                Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
            };
            if request.versioned_hashes.len() > MAX_BLOB_LIMIT {
                return problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None)
            }
            return blob_response::<BlobsV4Response, _>(
                self.get_blobs_v4_metered(request.versioned_hashes, request.indices_bitarray),
            )
        }
        let request = match BlobsV1Request::from_ssz_bytes(&body) {
            Ok(request) => request,
            Err(_) => return problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", None),
        };
        if request.versioned_hashes.len() > MAX_BLOB_LIMIT {
            return problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None)
        }
        let hashes = request.versioned_hashes;
        match version {
            1 => blob_response::<BlobsV1Response, _>(self.get_blobs_v1_metered(hashes).map(Some)),
            2 => blob_response::<BlobsV2Response, _>(self.get_blobs_v2_metered(hashes)),
            3 => blob_response::<BlobsV3Response, _>(self.get_blobs_v3_metered(hashes)),
            _ => problem_response(STATUS_NOT_FOUND, "method-not-found", None),
        }
    }
}

struct EngineSszState<Api> {
    engine_api: Option<Api>,
    witness_handler: Option<Arc<dyn EngineSszWitness>>,
    witness_enabled: bool,
}

impl<Api: EngineSszApi> EngineSszState<Api> {
    fn update_witness_support(&mut self) {
        self.witness_enabled = self.witness_handler.is_some() &&
            self.engine_api.as_ref().is_some_and(EngineSszApi::supports_witness);
    }
}

async fn handle_engine_ssz_request<Api>(
    handle: EngineSszProxyHandle<Api>,
    request: HttpRequest,
) -> HttpResponse
where
    Api: EngineSszApi,
{
    let Some(endpoint) = parse_engine_path(request.uri().path()) else {
        return problem_response(STATUS_NOT_FOUND, "method-not-found", None)
    };

    if request.method() != endpoint.method() {
        return problem_response(STATUS_METHOD_NOT_ALLOWED, "method-not-allowed", None)
    }

    match endpoint {
        EngineSszEndpoint::Capabilities => {
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.capabilities(handle.witness_enabled().await)
        }
        EngineSszEndpoint::Identity => {
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.identity()
        }
        EngineSszEndpoint::NewPayload => {
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
            let Some(fork) = request_fork(&request) else {
                return problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)
            };
            let (start, count) =
                match parse_bodies_range_query(request.uri().query().unwrap_or_default()) {
                    Ok(range) => range,
                    Err(response) => return response,
                };
            let Some(engine_api) = handle.engine_api().await else {
                return problem_response(STATUS_SERVICE_UNAVAILABLE, "service-unavailable", None)
            };
            engine_api.get_payload_bodies_by_range(fork, start, count).await
        }
        EngineSszEndpoint::Blobs(version) => {
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

fn parse_method_version(version: &str) -> Option<u8> {
    version.strip_prefix('v')?.parse().ok().filter(|version| (1..=4).contains(version))
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

impl EngineSszEndpoint {
    const fn method(&self) -> &'static str {
        match self {
            Self::Capabilities |
            Self::Identity |
            Self::GetPayload(_) |
            Self::PayloadBodiesByRange => "GET",
            Self::NewPayload |
            Self::PayloadsWithWitness |
            Self::Forkchoice |
            Self::PayloadBodiesByHash |
            Self::Blobs(_) => "POST",
        }
    }
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

async fn submit_payload<Provider, Pool, Validator, ChainSpec>(
    engine_api: &EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    fork: EngineSszFork,
    payload: ExecutionData,
) -> Result<EngineSszPayloadStatus, HttpResponse>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    let status = match fork.payloads_version() {
        1 => engine_api.new_payload_v1(payload).await,
        2 => engine_api.new_payload_v2(payload).await,
        3 => engine_api.new_payload_v3(payload).await,
        4 => engine_api.new_payload_v4(payload).await,
        5 => engine_api.new_payload_v5(payload).await,
        _ => return Err(problem_response(STATUS_BAD_REQUEST, "unsupported-fork", None)),
    }
    .map_err(engine_error_response)?;
    EngineSszPayloadStatus::try_from(status).map_err(|error| {
        problem_response(STATUS_INTERNAL_SERVER_ERROR, "internal", Some(error.to_string()))
    })
}

fn engine_error_response(err: EngineApiError) -> HttpResponse {
    let detail = err.to_string();
    let error: jsonrpsee::types::ErrorObjectOwned = err.into();
    let (status, problem_type) = match error.code() {
        -32700 => (STATUS_BAD_REQUEST, "parse-error"),
        -32600 => (STATUS_BAD_REQUEST, "invalid-request"),
        -32601 => (STATUS_NOT_FOUND, "method-not-found"),
        -32602 => (STATUS_UNPROCESSABLE_ENTITY, "invalid-body"),
        -38001 => (STATUS_NOT_FOUND, "unknown-payload"),
        -38002 => (STATUS_CONFLICT, "invalid-forkchoice"),
        -38003 => (STATUS_UNPROCESSABLE_ENTITY, "invalid-attributes"),
        -38004 => (STATUS_PAYLOAD_TOO_LARGE, "request-too-large"),
        -38005 => (STATUS_BAD_REQUEST, "unsupported-fork"),
        -38006 => (STATUS_CONFLICT, "reorg-too-deep"),
        _ => (STATUS_INTERNAL_SERVER_ERROR, "internal"),
    };
    problem_response(status, problem_type, Some(detail))
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
            engine_api.get_payload_bodies_by_hash_with_timestamps_metered(hashes, include_bal).await
        }
        PayloadBodiesRequest::Range { start, count } => {
            engine_api
                .get_payload_bodies_by_range_with_timestamps_metered(start, count, include_bal)
                .await
        }
    };
    let chain_spec = engine_api.chain_spec().as_ref();
    match fork {
        EngineSszFork::Amsterdam => payload_bodies_http_response(
            response,
            |body| ExecutionPayloadBodyAmsterdam::try_from(body).ok(),
            fork,
            chain_spec,
        ),
        EngineSszFork::Paris => payload_bodies_http_response(
            response,
            |body| ExecutionPayloadBodyParis::try_from(ExecutionPayloadBodyV1::from(body)).ok(),
            fork,
            chain_spec,
        ),
        _ => payload_bodies_http_response(
            response,
            |body| ExecutionPayloadBodyShanghai::try_from(ExecutionPayloadBodyV1::from(body)).ok(),
            fork,
            chain_spec,
        ),
    }
}

fn parse_bodies_range_query(query: &str) -> Result<(u64, u64), HttpResponse> {
    let invalid = || problem_response(STATUS_BAD_REQUEST, "invalid-request", None);
    let mut start = None;
    let mut count = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or_else(invalid)?;
        let field = match key {
            "from" if start.is_none() => &mut start,
            "count" if count.is_none() => &mut count,
            _ => return Err(invalid()),
        };
        *field = Some(value.parse::<u64>().map_err(|_| invalid())?);
    }
    let range = (start.ok_or_else(invalid)?, count.ok_or_else(invalid)?);
    if range.1 > MAX_BODIES_REQUEST as u64 {
        return Err(problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None))
    }
    Ok(range)
}

/// Amsterdam bodies with a missing or pruned BAL are unavailable, just like pruned blocks.
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
            body.filter(|(timestamp, _)| timestamp_matches_fork(chain_spec, fork, *timestamp))
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

fn timestamp_matches_fork<ChainSpec: EthereumHardforks>(
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

async fn read_ssz_body(request: HttpRequest, max_bytes: usize) -> Result<Bytes, HttpResponse> {
    let content_type = request.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok());
    if content_type != Some(OCTET_STREAM) {
        return Err(problem_response(STATUS_UNSUPPORTED_MEDIA_TYPE, "unsupported-media-type", None))
    }

    if let Some(content_length) = request.headers().get("content-length") {
        let Some(content_length) =
            content_length.to_str().ok().and_then(|value| value.parse::<usize>().ok())
        else {
            return Err(problem_response(STATUS_BAD_REQUEST, "invalid-request", None))
        };
        if content_length > max_bytes {
            return Err(problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None))
        }
    }

    match Limited::new(request.into_body(), max_bytes).collect().await {
        Ok(body) => Ok(body.to_bytes().into()),
        Err(err) if err.downcast_ref::<LengthLimitError>().is_some() => {
            Err(problem_response(STATUS_PAYLOAD_TOO_LARGE, "request-too-large", None))
        }
        Err(_) => Err(problem_response(STATUS_BAD_REQUEST, "invalid-request", None)),
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

/// Structural SSZ errors and invalid values have distinct REST error codes.
#[derive(Debug)]
enum PayloadDecodeError {
    Ssz(ssz::DecodeError),
    InvalidTransaction(alloy_eips::eip2718::Eip2718Error),
}

impl From<ssz::DecodeError> for PayloadDecodeError {
    fn from(error: ssz::DecodeError) -> Self {
        Self::Ssz(error)
    }
}

impl PayloadDecodeError {
    fn into_response(self) -> HttpResponse {
        match self {
            Self::Ssz(error) => {
                problem_response(STATUS_BAD_REQUEST, "ssz-decode-error", Some(format!("{error:?}")))
            }
            Self::InvalidTransaction(error) => problem_response(
                STATUS_UNPROCESSABLE_ENTITY,
                "invalid-body",
                Some(error.to_string()),
            ),
        }
    }
}

fn check_ssz_bound(actual: usize, max: usize, field: &str) -> Result<(), ssz::DecodeError> {
    if actual > max {
        return Err(ssz::DecodeError::BytesInvalid(format!(
            "{field} exceeds SSZ bound: {actual} > {max}"
        )))
    }
    Ok(())
}

fn decode_new_payload_request(
    fork: EngineSszFork,
    body: &[u8],
) -> Result<ExecutionData, PayloadDecodeError> {
    let (payload, parent_root, requests): (ExecutionPayload, _, Option<Requests>) = match fork {
        EngineSszFork::Paris => {
            let envelope = ExecutionPayloadEnvelopeParis::from_ssz_bytes(body)?;
            (envelope.payload.into(), None, None)
        }
        EngineSszFork::Shanghai => {
            let envelope = ExecutionPayloadEnvelopeShanghai::from_ssz_bytes(body)?;
            (envelope.payload.into(), None, None)
        }
        EngineSszFork::Cancun => {
            let envelope = ExecutionPayloadEnvelopeCancun::from_ssz_bytes(body)?;
            (envelope.payload.into(), Some(envelope.parent_beacon_block_root), None)
        }
        EngineSszFork::Prague => {
            let envelope = ExecutionPayloadEnvelopePrague::from_ssz_bytes(body)?;
            (
                envelope.payload.into(),
                Some(envelope.parent_beacon_block_root),
                Some(envelope.execution_requests),
            )
        }
        EngineSszFork::Osaka => {
            let envelope = ExecutionPayloadEnvelopeOsaka::from_ssz_bytes(body)?;
            (
                envelope.payload.into(),
                Some(envelope.parent_beacon_block_root),
                Some(envelope.execution_requests),
            )
        }
        EngineSszFork::Amsterdam => {
            let envelope = ExecutionPayloadEnvelopeAmsterdam::from_ssz_bytes(body)?;
            (
                ExecutionPayload::V4(envelope.payload),
                Some(envelope.parent_beacon_block_root),
                Some(envelope.execution_requests),
            )
        }
    };

    // Alloy's codecs reproduce the wire layout but do not enforce the SSZ list bounds.
    // Check those independently of the smaller, advertised total HTTP request limit.
    let inner = payload.as_v1();
    check_ssz_bound(inner.extra_data.len(), 32, "extra_data")?;
    check_ssz_bound(inner.transactions.len(), 1 << 20, "transactions")?;
    for transaction in &inner.transactions {
        check_ssz_bound(transaction.len(), 1 << 30, "transaction")?;
    }
    if let Some(withdrawals) = payload.withdrawals() {
        check_ssz_bound(withdrawals.len(), 16, "withdrawals")?;
    }
    if let Some(bal) = payload.block_access_list() {
        check_ssz_bound(bal.len(), 1 << 30, "block_access_list")?;
    }
    if let Some(requests) = &requests {
        check_ssz_bound(requests.len(), 256, "execution_requests")?;
        for request in requests.iter() {
            check_ssz_bound(request.len(), 1 << 30, "execution_request")?;
        }
    }

    let versioned_hashes = calculate_versioned_hashes(&inner.transactions)?;
    let sidecar = match parent_root {
        Some(parent_beacon_block_root) => {
            let cancun = CancunPayloadFields { parent_beacon_block_root, versioned_hashes };
            match requests {
                Some(requests) => {
                    ExecutionPayloadSidecar::v4(cancun, PraguePayloadFields::new(requests))
                }
                None => ExecutionPayloadSidecar::v3(cancun),
            }
        }
        None => ExecutionPayloadSidecar::none(),
    };
    Ok(ExecutionData::new(payload, sidecar))
}

fn calculate_versioned_hashes(transactions: &[Bytes]) -> Result<Vec<B256>, PayloadDecodeError> {
    let mut versioned_hashes = Vec::new();
    for transaction in transactions {
        let transaction = TxEnvelope::decode_2718_exact(transaction.as_ref())
            .map_err(PayloadDecodeError::InvalidTransaction)?;
        if let Some(hashes) = transaction.blob_versioned_hashes() {
            versioned_hashes.extend_from_slice(hashes);
        }
    }
    Ok(versioned_hashes)
}

fn decode_forkchoice_request(
    fork: EngineSszFork,
    body: &[u8],
) -> Result<(ForkchoiceState, Option<PayloadAttributes>, Option<B128>), ssz::DecodeError> {
    let (state, attrs, custody) = match fork {
        EngineSszFork::Paris => {
            let ForkchoiceUpdateParis { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateParis::from_ssz_bytes(body)?;
            (forkchoice_state, optional_attrs(payload_attributes), None)
        }
        EngineSszFork::Shanghai => {
            let ForkchoiceUpdateShanghai { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateShanghai::from_ssz_bytes(body)?;
            (forkchoice_state, optional_attrs(payload_attributes), None)
        }
        EngineSszFork::Cancun => {
            let ForkchoiceUpdateCancun { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateCancun::from_ssz_bytes(body)?;
            (forkchoice_state, optional_attrs(payload_attributes), None)
        }
        EngineSszFork::Prague => {
            let ForkchoiceUpdatePrague { forkchoice_state, payload_attributes } =
                ForkchoiceUpdatePrague::from_ssz_bytes(body)?;
            (forkchoice_state, optional_attrs(payload_attributes), None)
        }
        EngineSszFork::Osaka => {
            let ForkchoiceUpdateOsaka { forkchoice_state, payload_attributes } =
                ForkchoiceUpdateOsaka::from_ssz_bytes(body)?;
            (forkchoice_state, optional_attrs(payload_attributes), None)
        }
        EngineSszFork::Amsterdam => {
            let ForkchoiceUpdateAmsterdam { forkchoice_state, payload_attributes, custody_columns } =
                ForkchoiceUpdateAmsterdam::from_ssz_bytes(body)?;
            (forkchoice_state, optional_attrs(payload_attributes), custody_columns.into_option())
        }
    };
    if let Some(withdrawals) =
        attrs.as_ref().and_then(|attrs: &PayloadAttributes| attrs.withdrawals.as_ref())
    {
        check_ssz_bound(withdrawals.len(), 16, "withdrawals")?;
    }
    Ok((state, attrs, custody))
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
    let mut response = ssz_response(value);
    response.headers_mut().insert(CACHE_CONTROL, "no-store".parse().expect("valid cache control"));
    response
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
    async fn witness_capabilities_follow_both_wiring_orders() {
        #[derive(Clone)]
        struct Api(bool);
        impl EngineSszApi for Api {
            fn supports_witness(&self) -> bool {
                self.0
            }
        }
        struct Witness;
        impl EngineSszWitness for Witness {
            fn generate_witness(
                &self,
                _: ExecutionData,
            ) -> BoxFuture<
                'static,
                Result<crate::engine_ssz_containers::ExecutionWitnessV1, EngineSszWitnessError>,
            > {
                Box::pin(async { Ok(Default::default()) })
            }
        }
        let handle = EngineSszProxyHandle::new();
        handle.set_witness_handler_sync(Arc::new(Witness));
        assert!(!handle.witness_enabled().await);
        handle.set_engine_api_sync(Api(true));
        assert!(handle.witness_enabled().await);
        handle.set_engine_api(Api(false)).await;
        assert!(!handle.witness_enabled().await);

        let handle = EngineSszProxyHandle::with_engine_api(Api(true));
        assert!(!handle.witness_enabled().await);
        handle.set_witness_handler(Arc::new(Witness)).await;
        assert!(handle.witness_enabled().await);
    }

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

    #[test]
    fn payload_schema_bounds() {
        use alloy_rpc_types_engine::{ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3};
        let mut payload = ExecutionPayloadV3 {
            payload_inner: ExecutionPayloadV2 {
                payload_inner: ExecutionPayloadV1::from_block_unchecked(
                    B256::ZERO,
                    &reth_ethereum_primitives::Block::default(),
                ),
                withdrawals: vec![],
            },
            blob_gas_used: 0,
            excess_blob_gas: 0,
        };
        payload.payload_inner.payload_inner.extra_data = vec![0; 32].into();
        payload.payload_inner.withdrawals = vec![Default::default(); 16];
        let mut envelope = ExecutionPayloadEnvelopePrague {
            payload,
            parent_beacon_block_root: B256::ZERO,
            execution_requests: Requests::new(vec![Bytes::new(); 256]),
        };
        assert!(decode_new_payload_request(EngineSszFork::Prague, &envelope.as_ssz_bytes()).is_ok());
        envelope.execution_requests = Requests::new(vec![Bytes::new(); 257]);
        assert!(matches!(
            decode_new_payload_request(EngineSszFork::Prague, &envelope.as_ssz_bytes()),
            Err(PayloadDecodeError::Ssz(_))
        ));
        envelope.execution_requests = Requests::default();
        envelope.payload.payload_inner.withdrawals.push(Default::default());
        assert!(matches!(
            decode_new_payload_request(EngineSszFork::Prague, &envelope.as_ssz_bytes()),
            Err(PayloadDecodeError::Ssz(_))
        ));
        envelope.payload.payload_inner.withdrawals.clear();
        envelope.payload.payload_inner.payload_inner.extra_data = vec![0; 33].into();
        assert!(matches!(
            decode_new_payload_request(EngineSszFork::Prague, &envelope.as_ssz_bytes()),
            Err(PayloadDecodeError::Ssz(_))
        ));
        let shanghai = ExecutionPayloadEnvelopeShanghai {
            payload: ExecutionPayloadV2 {
                payload_inner: ExecutionPayloadV1::from_block_unchecked(
                    B256::ZERO,
                    &reth_ethereum_primitives::Block::default(),
                ),
                withdrawals: vec![Default::default(); 17],
            },
        };
        assert!(matches!(
            decode_new_payload_request(EngineSszFork::Shanghai, &shanghai.as_ssz_bytes()),
            Err(PayloadDecodeError::Ssz(_))
        ));
        let paris = ExecutionPayloadEnvelopeParis {
            payload: ExecutionPayloadV1 {
                transactions: vec![Bytes::new(); (1 << 20) + 1],
                ..ExecutionPayloadV1::from_block_unchecked(
                    B256::ZERO,
                    &reth_ethereum_primitives::Block::default(),
                )
            },
        };
        assert!(matches!(
            decode_new_payload_request(EngineSszFork::Paris, &paris.as_ssz_bytes()),
            Err(PayloadDecodeError::Ssz(_))
        ));
    }

    #[test]
    fn forkchoice_withdrawal_bounds() {
        use crate::engine_ssz_containers::{
            PayloadAttributesAmsterdam, PayloadAttributesCancun, PayloadAttributesShanghai,
        };
        for count in [16, 17] {
            let withdrawals = vec![Default::default(); count];
            let state = ForkchoiceState::default();
            let shanghai = ForkchoiceUpdateShanghai {
                forkchoice_state: state,
                payload_attributes: Optional::some(PayloadAttributesShanghai {
                    withdrawals: withdrawals.clone(),
                    ..Default::default()
                }),
            };
            let cancun = ForkchoiceUpdateCancun {
                forkchoice_state: state,
                payload_attributes: Optional::some(PayloadAttributesCancun {
                    withdrawals: withdrawals.clone(),
                    ..Default::default()
                }),
            };
            let amsterdam = ForkchoiceUpdateAmsterdam {
                forkchoice_state: state,
                payload_attributes: Optional::some(PayloadAttributesAmsterdam {
                    withdrawals,
                    ..Default::default()
                }),
                custody_columns: Optional::none(),
            };
            for (fork, bytes) in [
                (EngineSszFork::Shanghai, shanghai.as_ssz_bytes()),
                (EngineSszFork::Cancun, cancun.as_ssz_bytes()),
                (EngineSszFork::Prague, cancun.as_ssz_bytes()),
                (EngineSszFork::Osaka, cancun.as_ssz_bytes()),
                (EngineSszFork::Amsterdam, amsterdam.as_ssz_bytes()),
            ] {
                assert_eq!(
                    decode_forkchoice_request(fork, &bytes).is_ok(),
                    count == 16,
                    "{fork:?}"
                );
            }
        }
    }

    #[test]
    fn paris_forkchoice_uses_fixed_size_optional_attributes() {
        use crate::engine_ssz_containers::PayloadAttributesParis;
        let request = ForkchoiceUpdateParis {
            forkchoice_state: ForkchoiceState::default(),
            payload_attributes: Optional::some(PayloadAttributesParis {
                timestamp: 1_700_000_000,
                ..Default::default()
            }),
        };
        let bytes = request.as_ssz_bytes();
        assert_eq!(bytes.len(), 160);
        assert_eq!(&bytes[96..100], &100u32.to_le_bytes());
        assert_eq!(&bytes[100..108], &1_700_000_000u64.to_le_bytes());
        assert_eq!(
            decode_forkchoice_request(EngineSszFork::Paris, &bytes).unwrap().1.unwrap().timestamp,
            1_700_000_000
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
    fn bodies_range_query_validation() {
        assert_eq!(parse_bodies_range_query("from=1&count=32").unwrap(), (1, 32));
        assert_eq!(parse_bodies_range_query("count=2&from=3").unwrap(), (3, 2));
        for query in
            ["", "from=1", "from=x&count=1", "from=1&count=1&count=2", "from=1&count=1&other=0"]
        {
            assert_eq!(parse_bodies_range_query(query).unwrap_err().status(), 400);
        }
        assert_eq!(parse_bodies_range_query("from=1&count=33").unwrap_err().status(), 413);
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
        assert!(timestamp_matches_fork(&chain_spec, EngineSszFork::Paris, 9));
        assert!(!timestamp_matches_fork(&chain_spec, EngineSszFork::Paris, 10));
        assert!(!timestamp_matches_fork(&chain_spec, EngineSszFork::Amsterdam, 49));
        assert!(timestamp_matches_fork(&chain_spec, EngineSszFork::Amsterdam, 50));
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
