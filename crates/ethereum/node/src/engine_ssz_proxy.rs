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
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadSidecar,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ExecutionPayloadV4,
    ForkchoiceState, PayloadAttributes, PraguePayloadFields,
};
use http_body_util::BodyExt;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::EngineApiValidator;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_provider::{BalProvider, BlockReader, HeaderProvider, StateProviderFactory};
use reth_rpc::EngineApi;
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
const TEXT_PLAIN: &str = "text/plain";
const CONTENT_TYPE: &str = "content-type";

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_SERVICE_UNAVAILABLE: u16 = 503;

const MAX_BLOB_LIMIT: usize = 128;
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

type EthEngineApi<Provider, Pool, Validator, ChainSpec> =
    EngineApi<Provider, EthEngineTypes, Pool, Validator, ChainSpec>;
type SharedEthEngineApi<Provider, Pool, Validator, ChainSpec> =
    Arc<RwLock<Option<EthEngineApi<Provider, Pool, Validator, ChainSpec>>>>;

/// Shared handle used by [`EngineSszProxyLayer`].
pub struct EngineSszProxyHandle<ChainSpec, Provider = (), Pool = (), Validator = ()> {
    engine_api: SharedEthEngineApi<Provider, Pool, Validator, ChainSpec>,
}

impl<C, Provider, Pool, Validator> Clone for EngineSszProxyHandle<C, Provider, Pool, Validator> {
    fn clone(&self) -> Self {
        Self { engine_api: self.engine_api.clone() }
    }
}

impl<C, Provider, Pool, Validator> std::fmt::Debug
    for EngineSszProxyHandle<C, Provider, Pool, Validator>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSszProxyHandle").finish_non_exhaustive()
    }
}

impl<ChainSpec, Provider, Pool, Validator>
    EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>
{
    fn new() -> Self {
        Self { engine_api: Default::default() }
    }

    fn with_engine_api(engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>) -> Self {
        Self { engine_api: Arc::new(RwLock::new(Some(engine_api))) }
    }

    /// Sets the Engine API implementation used by the proxy.
    pub async fn set_engine_api(
        &self,
        engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    ) {
        *self.engine_api.write().await = Some(engine_api);
    }

    /// Sets the Engine API implementation during synchronous launch wiring.
    pub fn set_engine_api_sync(
        &self,
        engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    ) {
        *self
            .engine_api
            .try_write()
            .expect("engine api handle should not be locked during launch") = Some(engine_api);
    }
}

impl<ChainSpec, Provider, Pool, Validator>
    EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>
{
    /// Returns the Engine API implementation used by the proxy.
    pub async fn engine_api(&self) -> Option<EthEngineApi<Provider, Pool, Validator, ChainSpec>> {
        self.engine_api.read().await.clone()
    }
}

/// A tower layer that intercepts SSZ Engine API routes under `/engine/v2`.
#[derive(Clone, Debug)]
pub struct EngineSszProxyLayer<ChainSpec, Provider = (), Pool = (), Validator = ()> {
    handle: EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>,
}

impl<ChainSpec, Provider, Pool, Validator>
    EngineSszProxyLayer<ChainSpec, Provider, Pool, Validator>
{
    /// Creates a new proxy layer and a handle for setting the engine after node launch.
    pub fn new() -> (Self, EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>) {
        let handle = EngineSszProxyHandle::new();
        (Self { handle: handle.clone() }, handle)
    }

    /// Creates a new proxy layer with an Engine API implementation.
    pub fn with_engine_api(
        engine_api: EthEngineApi<Provider, Pool, Validator, ChainSpec>,
    ) -> (Self, EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>) {
        let handle = EngineSszProxyHandle::with_engine_api(engine_api);
        (Self { handle: handle.clone() }, handle)
    }
}

impl<S, ChainSpec, Provider, Pool, Validator> Layer<S>
    for EngineSszProxyLayer<ChainSpec, Provider, Pool, Validator>
{
    type Service = EngineSszProxyService<S, ChainSpec, Provider, Pool, Validator>;

    fn layer(&self, inner: S) -> Self::Service {
        EngineSszProxyService { inner, handle: self.handle.clone() }
    }
}

/// The service produced by [`EngineSszProxyLayer`].
#[derive(Clone, Debug)]
pub struct EngineSszProxyService<S, ChainSpec, Provider = (), Pool = (), Validator = ()> {
    inner: S,
    handle: EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>,
}

impl<S, ChainSpec, Provider, Pool, Validator> Service<HttpRequest>
    for EngineSszProxyService<S, ChainSpec, Provider, Pool, Validator>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError> + Send + Clone,
    S::Future: Send + 'static,
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
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

async fn handle_engine_ssz_request<ChainSpec, Provider, Pool, Validator>(
    handle: EngineSszProxyHandle<ChainSpec, Provider, Pool, Validator>,
    request: HttpRequest,
) -> HttpResponse
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + BalProvider + 'static,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EthEngineTypes>,
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
            let Some(engine_api) = handle.engine_api().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine api unavailable")
            };
            handle_identity(engine_api)
        }
        EngineSszEndpoint::Payloads(fork) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine_api) = handle.engine_api().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine api unavailable")
            };
            handle_new_payload(engine_api, fork.payloads_version(), &body).await
        }
        EngineSszEndpoint::Forkchoice(fork) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine_api) = handle.engine_api().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine api unavailable")
            };
            handle_forkchoice_updated(engine_api, fork.forkchoice_version(), &body).await
        }
        EngineSszEndpoint::Blobs(version) => {
            if method != "POST" {
                return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
            }
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine_api) = handle.engine_api().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine api unavailable")
            };
            handle_get_blobs(engine_api, version, &body).await
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

async fn handle_new_payload<Provider, Pool, Validator, ChainSpec>(
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
    let payload = match decode_new_payload_request(version, body) {
        Ok(payload) => payload,
        Err(err) => return text_response(STATUS_BAD_REQUEST, err),
    };

    let response = match version {
        1 => engine_api.new_payload_v1(payload).await,
        2 => engine_api.new_payload_v2(payload).await,
        3 => engine_api.new_payload_v3(payload).await,
        4 => engine_api.new_payload_v4(payload).await,
        5 => engine_api.new_payload_v5(payload).await,
        _ => return text_response(STATUS_BAD_REQUEST, "unsupported payload endpoint version"),
    };

    match response {
        Ok(status) => ssz_response(status),
        Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn handle_forkchoice_updated<Provider, Pool, Validator, ChainSpec>(
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
    let (state, attrs, custody_columns) = match decode_forkchoice_request(version, body) {
        Ok(request) => request,
        Err(err) => return text_response(STATUS_BAD_REQUEST, err),
    };

    let response = match version {
        1 => engine_api.fork_choice_updated_v1_metered(state, attrs).await,
        2 => engine_api.fork_choice_updated_v2_metered(state, attrs).await,
        3 => engine_api.fork_choice_updated_v3_metered(state, attrs).await,
        4 => engine_api.fork_choice_updated_v4_metered(state, attrs, custody_columns).await,
        _ => return text_response(STATUS_BAD_REQUEST, "unsupported forkchoice endpoint version"),
    };

    match response {
        Ok(updated) => ssz_response(updated),
        Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
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
    match version {
        1 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match engine_api.get_blobs_v1_metered(hashes) {
                Ok(response) => ssz_response(response),
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        2 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match engine_api.get_blobs_v2_metered(hashes) {
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
            match engine_api.get_blobs_v3_metered(hashes) {
                Ok(Some(response)) => ssz_response(response),
                Ok(None) => no_content_response(),
                Err(err) => text_response(STATUS_INTERNAL_SERVER_ERROR, err.to_string()),
            }
        }
        4 => {
            let (hashes, indices_bitarray) = match decode_blob_cells_request(body) {
                Ok(request) => request,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match engine_api.get_blobs_v4_metered(hashes, indices_bitarray) {
                Ok(Some(response)) => ssz_response(response),
                Ok(None) => no_content_response(),
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
) -> Result<(ForkchoiceState, Option<PayloadAttributes>, Option<B128>), &'static str> {
    match version {
        1..=3 => {
            let (forkchoice_state, payload_attributes) =
                <(ForkchoiceState, Vec<PayloadAttributes>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            Ok((forkchoice_state, payload_attrs(version, payload_attributes)?, None))
        }
        4 => {
            let (forkchoice_state, payload_attributes, custody_columns) =
                <(ForkchoiceState, Vec<PayloadAttributes>, Vec<B128>)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            Ok((
                forkchoice_state,
                payload_attrs(version, payload_attributes)?,
                custody_columns_opt(custody_columns)?,
            ))
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

        let (decoded_state, decoded_attrs, custody_columns) =
            decode_forkchoice_request(4, &encoded).unwrap();
        assert_eq!(decoded_state, forkchoice_state);
        assert!(decoded_attrs.is_none());
        assert_eq!(custody_columns, Some(B128::with_last_byte(1)));
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
