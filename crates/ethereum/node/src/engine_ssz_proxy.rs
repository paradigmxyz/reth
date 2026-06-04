//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v2`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{
    eip2718::Decodable2718,
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
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
use reth_engine_primitives::ConsensusEngineHandle;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_payload_primitives::EngineObjectValidationError;
use reth_rpc_engine_api::{EngineApiError, EngineApiResult};
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
const TEXT_PLAIN: &str = "text/plain";
const CONTENT_TYPE: &str = "content-type";

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_SERVICE_UNAVAILABLE: u16 = 503;

const MAX_BLOB_LIMIT: usize = 128;

/// Shared handle used by [`EngineSszProxyLayer`].
pub struct EngineSszProxyHandle<ChainSpec> {
    engine: Arc<RwLock<Option<ConsensusEngineHandle<EthEngineTypes>>>>,
    blob_store: Arc<RwLock<Option<Arc<dyn BlobStore>>>>,
    chain_spec: Arc<ChainSpec>,
}

impl<C> Clone for EngineSszProxyHandle<C> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            blob_store: self.blob_store.clone(),
            chain_spec: self.chain_spec.clone(),
        }
    }
}

impl<C> std::fmt::Debug for EngineSszProxyHandle<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSszProxyHandle").finish_non_exhaustive()
    }
}

impl<C> Default for EngineSszProxyHandle<C> {
    fn default() -> Self {
        Self {
            engine: Default::default(),
            blob_store: Default::default(),
            chain_spec: Default::default(),
        }
    }
}

impl<ChainSpec> EngineSszProxyHandle<ChainSpec> {
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

    /// Sets the chain spec used for getBlobs fork validation.
    pub async fn set_chain_spec(&self, chain_spec: Arc<ChainSpec>) {
        self.chain_spec = chain_spec;
    }

    async fn blob_store(&self) -> Option<Arc<dyn BlobStore>> {
        self.blob_store.read().await.clone()
    }

    async fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec
    }
}

/// A tower layer that intercepts SSZ Engine API routes under `/engine/v2`.
#[derive(Clone, Debug, Default)]
pub struct EngineSszProxyLayer<ChainSpec> {
    handle: EngineSszProxyHandle<ChainSpec>,
}

impl<ChainSpec> EngineSszProxyLayer<ChainSpec> {
    /// Creates a new proxy layer and a handle for setting the engine after node launch.
    pub fn new() -> (Self, EngineSszProxyHandle<ChainSpec>) {
        let handle = EngineSszProxyHandle::default();
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

async fn handle_engine_ssz_request(
    handle: EngineSszProxyHandle<impl EthereumHardforks + Send + Sync + 'static>,
    request: HttpRequest,
) -> HttpResponse {
    if request.method().as_str() != "POST" {
        return text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed")
    }

    let path = request.uri().path().to_owned();
    let Some(endpoint) = parse_engine_path(&path) else {
        return text_response(STATUS_NOT_FOUND, "unknown engine ssz endpoint")
    };

    let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
        return text_response(STATUS_BAD_REQUEST, "failed to read request body")
    };

    match endpoint {
        EngineSszEndpoint::Payloads(fork) => {
            let Some(engine) = handle.engine().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine handle unavailable")
            };
            handle_new_payload(engine, fork.payloads_version(), &body).await
        }
        EngineSszEndpoint::Forkchoice(fork) => {
            let Some(engine) = handle.engine().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine handle unavailable")
            };
            handle_forkchoice_updated(engine, fork.forkchoice_version(), &body).await
        }
        EngineSszEndpoint::Blobs(version) => handle_get_blobs(handle, version, &body).await,
    }
}

fn parse_engine_path(path: &str) -> Option<EngineSszEndpoint> {
    let mut segments = path.trim_start_matches('/').split('/');
    match (segments.next(), segments.next(), segments.next(), segments.next(), segments.next()) {
        (Some("engine"), Some("v2"), Some(fork), Some("payloads"), None) => {
            Some(EngineSszEndpoint::Payloads(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("forkchoice"), None) => {
            Some(EngineSszEndpoint::Forkchoice(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("blobs"), None) => {
            Some(EngineSszEndpoint::Blobs(fork.parse::<EngineSszFork>().ok()?.blobs_version()?))
        }
        (Some("engine"), version, Some("blobs"), None, None) => {
            Some(EngineSszEndpoint::Blobs(parse_method_version(version?)?))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EngineSszEndpoint {
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

    const fn blobs_version(self) -> Option<u8> {
        match self {
            Self::Paris | Self::Shanghai => None,
            Self::Cancun | Self::Prague => Some(1),
            Self::Osaka => Some(3),
            Self::Amsterdam => Some(4),
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

impl<C> EngineSszProxyHandle<C>
where
    C: EthereumHardforks,
{
    async fn get_blobs_v1(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Vec<Option<BlobAndProofV1>>> {
        let chain_spec = self.chain_spec().await.ok_or_else(engine_unavailable)?;
        let current_timestamp = current_timestamp();
        if chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
            return Err(unsupported_fork())
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        self.blob_store()
            .await
            .ok_or_else(engine_unavailable)?
            .get_by_versioned_hashes_v1(&versioned_hashes)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }

    async fn get_blobs_v2(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Option<Vec<BlobAndProofV2>>> {
        let chain_spec = self.chain_spec().await.ok_or_else(engine_unavailable)?;
        let current_timestamp = current_timestamp();
        if !chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
            return Err(unsupported_fork())
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        self.blob_store()
            .await
            .ok_or_else(engine_unavailable)?
            .get_by_versioned_hashes_v2(&versioned_hashes)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }

    async fn get_blobs_v3(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> EngineApiResult<Option<Vec<Option<BlobAndProofV2>>>> {
        let chain_spec = self.chain_spec().await.ok_or_else(engine_unavailable)?;
        let current_timestamp = current_timestamp();
        if !chain_spec.is_osaka_active_at_timestamp(current_timestamp) {
            return Err(unsupported_fork())
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        // // Spec requires returning `null` if syncing.
        // if self.is_syncing().await {
        //     return Ok(None)
        // }

        self.blob_store()
            .await
            .ok_or_else(engine_unavailable)?
            .get_by_versioned_hashes_v3(&versioned_hashes)
            .map(Some)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }

    async fn get_blobs_v4(
        &self,
        versioned_hashes: Vec<B256>,
        indices_bitarray: B128,
    ) -> EngineApiResult<Option<Vec<Option<BlobCellsAndProofsV1>>>> {
        let chain_spec = self.chain_spec().await.ok_or_else(engine_unavailable)?;
        let current_timestamp = current_timestamp();
        if !chain_spec.is_amsterdam_active_at_timestamp(current_timestamp) {
            return Err(unsupported_fork())
        }

        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() })
        }

        // // Spec requires returning `null` if syncing.
        // if self.is_syncing().await {
        //     return Ok(None)
        // }

        self.blob_store()
            .await
            .ok_or_else(engine_unavailable)?
            .get_by_versioned_hashes_v4(&versioned_hashes, indices_bitarray)
            .map(Some)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))
    }
}

/// Handles SSZ `engine_getBlobsV*` requests with the native Engine API blob provider.
async fn handle_get_blobs(
    handler: EngineSszProxyHandle<impl EthereumHardforks + Send + Sync + 'static>,
    version: u8,
    body: &[u8],
) -> HttpResponse {
    match version {
        1 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match handler.get_blobs_v1(hashes).await {
                Ok(response) => ssz_response(response),
                Err(err) => engine_error_response(err),
            }
        }
        2 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match handler.get_blobs_v2(hashes).await {
                Ok(Some(response)) => ssz_response(response),
                Ok(None) => no_content_response(),
                Err(err) => engine_error_response(err),
            }
        }
        3 => {
            let hashes = match decode_blob_hashes_request(body) {
                Ok(hashes) => hashes,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match handler.get_blobs_v3(hashes).await {
                Ok(Some(response)) => ssz_response(response),
                Ok(None) => no_content_response(),
                Err(err) => engine_error_response(err),
            }
        }
        4 => {
            let (hashes, indices_bitarray) = match decode_blob_cells_request(body) {
                Ok(request) => request,
                Err(err) => return text_response(STATUS_BAD_REQUEST, err),
            };
            match handler.get_blobs_v4(hashes, indices_bitarray).await {
                Ok(Some(response)) => ssz_response(response),
                Ok(None) => no_content_response(),
                Err(err) => engine_error_response(err),
            }
        }
        _ => text_response(STATUS_NOT_FOUND, "unsupported blobs endpoint version"),
    }
}

/// Decodes the common getBlobs request container with only versioned hashes.
fn decode_blob_hashes_request(body: &[u8]) -> Result<Vec<B256>, &'static str> {
    decode_one::<Vec<B256>>(body).map_err(|_| "invalid ssz")
}

/// Decodes the Amsterdam getBlobs request container with hashes and a cell index mask.
fn decode_blob_cells_request(body: &[u8]) -> Result<(Vec<B256>, B128), &'static str> {
    <(Vec<B256>, B128) as ssz::Decode>::from_ssz_bytes(body).map_err(|_| "invalid ssz")
}

fn engine_error_response(error: EngineApiError) -> HttpResponse {
    let status = match error {
        EngineApiError::BlobRequestTooLarge { .. } => 413,
        EngineApiError::EngineObjectValidationError(
            EngineObjectValidationError::UnsupportedFork,
        ) => STATUS_BAD_REQUEST,
        _ => STATUS_INTERNAL_SERVER_ERROR,
    };
    text_response(status, error.to_string())
}

fn current_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn unsupported_fork() -> EngineApiError {
    EngineApiError::EngineObjectValidationError(EngineObjectValidationError::UnsupportedFork)
}

fn engine_unavailable() -> EngineApiError {
    EngineApiError::Internal(Box::new(std::io::Error::other("engine ssz blob context unavailable")))
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
    fn parses_blob_endpoints() {
        let endpoint = parse_engine_path("/engine/v2/amsterdam/blobs").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Blobs(4));

        let endpoint = parse_engine_path("/engine/v3/blobs").unwrap();
        assert_eq!(endpoint, EngineSszEndpoint::Blobs(3));
    }

    #[test]
    fn rejects_legacy_version_scoped_endpoint() {
        assert!(parse_engine_path("/engine/v4/payloads").is_none());
    }
}
