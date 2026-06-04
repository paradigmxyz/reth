//! HTTP SSZ transport proxy for the authenticated Engine API server.
//!
//! Implements the [EIP-8178] SSZ Engine API routes under `/engine/v2`.
//!
//! [EIP-8178]: https://eips.ethereum.org/EIPS/eip-8178

use alloy_eips::eip7685::{Requests, RequestsOrHash};
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5,
    ExecutionPayloadEnvelopeV6, ExecutionPayloadSidecar, ExecutionPayloadV1, ExecutionPayloadV2,
    ExecutionPayloadV3, ExecutionPayloadV4, ForkchoiceState, PayloadAttributes, PayloadId,
    PraguePayloadFields,
};
use http_body_util::BodyExt;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse};
use reth_engine_primitives::ConsensusEngineHandle;
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes};
use reth_payload_builder::PayloadStore;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
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

/// Shared handle used by [`EngineSszProxyLayer`].
#[derive(Clone, Debug, Default)]
pub struct EngineSszProxyHandle {
    engine: Arc<RwLock<Option<ConsensusEngineHandle<EthEngineTypes>>>>,
    payload_store: Arc<RwLock<Option<Arc<PayloadStore<EthEngineTypes>>>>>,
}

impl EngineSszProxyHandle {
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
}

/// A tower layer that intercepts SSZ Engine API routes under `/engine/v2`.
#[derive(Clone, Debug, Default)]
pub struct EngineSszProxyLayer {
    handle: EngineSszProxyHandle,
}

impl EngineSszProxyLayer {
    /// Creates a new proxy layer and a handle for setting the engine after node launch.
    pub fn new() -> (Self, EngineSszProxyHandle) {
        let handle = EngineSszProxyHandle::default();
        (Self { handle: handle.clone() }, handle)
    }
}

impl<S> Layer<S> for EngineSszProxyLayer {
    type Service = EngineSszProxyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        EngineSszProxyService { inner, handle: self.handle.clone() }
    }
}

/// The service produced by [`EngineSszProxyLayer`].
#[derive(Clone, Debug)]
pub struct EngineSszProxyService<S> {
    inner: S,
    handle: EngineSszProxyHandle,
}

impl<S> Service<HttpRequest> for EngineSszProxyService<S>
where
    S: Service<HttpRequest, Response = HttpResponse, Error = BoxError> + Send + Clone,
    S::Future: Send + 'static,
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

/// Supported Engine API SSZ routes, scoped by fork for versioning.
enum SszEngineApiRoute {
    /// Route for `engine_newPayloadV{1,2,3,4,5}` endpoints.
    NewPayload(EngineSszFork),
    /// Route for `engine_getPayloadV{1,2,3,4,5,6}` endpoints.
    GetPayload(EngineSszFork, PayloadId),
    /// Route for `engine_forkchoiceUpdatedV{1,2,3,4}` endpoints.
    ForkchoiceUpdated(EngineSszFork),
}

async fn handle_engine_ssz_request(
    handle: EngineSszProxyHandle,
    request: HttpRequest,
) -> HttpResponse {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let Some(route) = parse_engine_path(&path) else {
        return text_response(STATUS_NOT_FOUND, "unknown engine ssz endpoint")
    };

    match (method.as_str(), route) {
        ("POST", SszEngineApiRoute::NewPayload(fork)) => {
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine) = handle.engine().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine handle unavailable")
            };
            handle_new_payload(engine, fork.payloads_version(), &body).await
        }
        ("POST", SszEngineApiRoute::ForkchoiceUpdated(fork)) => {
            let Ok(body) = request.into_body().collect().await.map(|body| body.to_bytes()) else {
                return text_response(STATUS_BAD_REQUEST, "failed to read request body")
            };
            let Some(engine) = handle.engine().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "engine handle unavailable")
            };
            handle_forkchoice_updated(engine, fork.forkchoice_version(), &body).await
        }
        ("GET", SszEngineApiRoute::GetPayload(fork, payload_id)) => {
            let Some(payload_store) = handle.payload_store().await else {
                return text_response(STATUS_SERVICE_UNAVAILABLE, "payload store unavailable")
            };
            handle_get_payload(payload_store, fork.get_payload_version(), payload_id).await
        }
        _ => text_response(STATUS_METHOD_NOT_ALLOWED, "method not allowed"),
    }
}

fn parse_engine_path(path: &str) -> Option<SszEngineApiRoute> {
    let mut segments = path.trim_start_matches('/').split('/');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("engine"), Some("v2"), Some(fork), Some("payloads"), None, None) => {
            Some(SszEngineApiRoute::NewPayload(fork.parse().ok()?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("payloads"), Some(payload_id), None) => {
            Some(SszEngineApiRoute::GetPayload(fork.parse().ok()?, parse_payload_id(payload_id)?))
        }
        (Some("engine"), Some("v2"), Some(fork), Some("forkchoice"), None, None) => {
            Some(SszEngineApiRoute::ForkchoiceUpdated(fork.parse().ok()?))
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

fn decode_new_payload_request(version: u8, body: &[u8]) -> Result<ExecutionData, &'static str> {
    use ssz::Decode;

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
            let (execution_payload, expected_blob_versioned_hashes, parent_beacon_block_root) =
                <(ExecutionPayloadV3, Vec<B256>, B256)>::from_ssz_bytes(body)
                    .map_err(|_| "invalid ssz")?;
            let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
                parent_beacon_block_root,
                versioned_hashes: expected_blob_versioned_hashes,
            });
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        4 => {
            let (
                execution_payload,
                expected_blob_versioned_hashes,
                parent_beacon_block_root,
                execution_requests,
            ) = <(ExecutionPayloadV3, Vec<B256>, B256, Vec<Bytes>)>::from_ssz_bytes(body)
                .map_err(|_| "invalid ssz")?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root,
                    versioned_hashes: expected_blob_versioned_hashes,
                },
                PraguePayloadFields::new(RequestsOrHash::Requests(Requests::new(
                    execution_requests,
                ))),
            );
            Ok(ExecutionData::new(execution_payload.into(), sidecar))
        }
        5 => {
            let (
                execution_payload,
                expected_blob_versioned_hashes,
                parent_beacon_block_root,
                execution_requests,
            ) = <(ExecutionPayloadV4, Vec<B256>, B256, Vec<Bytes>)>::from_ssz_bytes(body)
                .map_err(|_| "invalid ssz")?;
            let sidecar = ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root,
                    versioned_hashes: expected_blob_versioned_hashes,
                },
                PraguePayloadFields::new(RequestsOrHash::Requests(Requests::new(
                    execution_requests,
                ))),
            );
            Ok(ExecutionData::new(ExecutionPayload::V4(execution_payload), sidecar))
        }
        _ => Err("unsupported payload endpoint version"),
    }
}

fn decode_forkchoice_request(
    version: u8,
    body: &[u8],
) -> Result<(ForkchoiceState, Option<PayloadAttributes>), &'static str> {
    use ssz::Decode;

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
        let SszEngineApiRoute::NewPayload(fork) =
            parse_engine_path("/engine/v2/prague/payloads").unwrap()
        else {
            panic!("expected new payload route")
        };
        assert_eq!(fork, EngineSszFork::Prague);
        assert_eq!(fork.payloads_version(), 4);
    }

    #[test]
    fn parses_fork_scoped_get_payload_endpoint() {
        let SszEngineApiRoute::GetPayload(fork, payload_id) =
            parse_engine_path("/engine/v2/prague/payloads/0x1234567890abcdef").unwrap()
        else {
            panic!("expected get payload route")
        };
        assert_eq!(fork, EngineSszFork::Prague);
        assert_eq!(fork.get_payload_version(), 4);
        assert_eq!(payload_id, PayloadId::new([0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]));
    }

    #[test]
    fn parses_fork_scoped_forkchoice_endpoint() {
        let SszEngineApiRoute::ForkchoiceUpdated(fork) =
            parse_engine_path("/engine/v2/amsterdam/forkchoice").unwrap()
        else {
            panic!("expected forkchoice route")
        };
        assert_eq!(fork, EngineSszFork::Amsterdam);
        assert_eq!(fork.forkchoice_version(), 4);
    }

    #[test]
    fn rejects_legacy_version_scoped_endpoint() {
        assert!(parse_engine_path("/engine/v4/payloads").is_none());
    }

    #[test]
    fn rejects_get_payload_endpoint_with_extra_segments() {
        assert!(parse_engine_path("/engine/v2/prague/payloads/0x1234567890abcdef/extra").is_none());
    }
}
