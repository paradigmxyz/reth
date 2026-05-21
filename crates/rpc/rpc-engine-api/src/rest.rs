//! REST handler for `POST /new-payload-with-witness`.
//!
//! This module provides a [`tower::Layer`] that intercepts HTTP requests to
//! `POST /new-payload-with-witness` on the authenticated Engine API server.
//! Non-matching requests are forwarded to the inner JSON-RPC service.
//!
//! See the [Engine API REST spec](https://github.com/ethereum/execution-apis/blob/main/src/engine/rest.md).

use crate::{ssz_types::encode_new_payload_with_witness_response, EngineApiError};
use alloy_eips::eip7685::RequestsOrHash;
use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayloadSidecar, ExecutionPayloadV4,
    PraguePayloadFields,
};
use http::{header, Method, Response, StatusCode};
use reth_payload_primitives::EngineObjectValidationError;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::{debug, trace, warn};

/// The REST route path.
const NEW_PAYLOAD_WITH_WITNESS_PATH: &str = "/new-payload-with-witness";

/// JSON error response body used for REST error responses.
#[derive(serde::Serialize)]
struct RestError {
    code: i32,
    message: String,
}

/// Creates a JSON error response with the given HTTP status code and engine error code.
fn json_error_response(
    http_status: StatusCode,
    code: i32,
    message: impl Into<String>,
) -> Response<http_body_util::Full<bytes::Bytes>> {
    let body = serde_json::to_vec(&RestError { code, message: message.into() }).unwrap_or_default();
    Response::builder()
        .status(http_status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::new(bytes::Bytes::from(body)))
        .expect("valid response")
}

/// Trait for types that can handle the new-payload-with-witness REST request.
///
/// This abstracts over the `EngineApi` to allow the REST handler to call
/// `new_payload_v5` without being generic over all the engine API type params.
pub trait NewPayloadWithWitnessHandler: Send + Sync + 'static {
    /// Executes `engine_newPayloadV5` logic and returns the payload status.
    fn new_payload_v5(
        &self,
        payload: ExecutionData,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<alloy_rpc_types_engine::PayloadStatus, EngineApiError>>
                + Send
                + '_,
        >,
    >;

    /// Returns whether the engine accepts execution requests hash.
    fn accept_execution_requests_hash(&self) -> bool;
}

/// A [`tower::Layer`] that adds the `POST /new-payload-with-witness` REST endpoint.
///
/// This layer wraps the inner HTTP service (typically the authenticated JSON-RPC server)
/// and intercepts requests to the REST path. All other requests pass through unchanged.
#[derive(Clone, Debug)]
pub struct NewPayloadWithWitnessLayer<H> {
    handler: Arc<H>,
}

impl<H> NewPayloadWithWitnessLayer<H> {
    /// Creates a new layer with the given handler.
    pub fn new(handler: Arc<H>) -> Self {
        Self { handler }
    }
}

impl<S, H> tower::Layer<S> for NewPayloadWithWitnessLayer<H>
where
    H: NewPayloadWithWitnessHandler,
{
    type Service = NewPayloadWithWitnessService<S, H>;

    fn layer(&self, inner: S) -> Self::Service {
        NewPayloadWithWitnessService { inner, handler: Arc::clone(&self.handler) }
    }
}

/// The [`tower::Service`] that handles `POST /new-payload-with-witness`.
#[derive(Clone, Debug)]
pub struct NewPayloadWithWitnessService<S, H> {
    inner: S,
    handler: Arc<H>,
}

impl<S, H, ReqBody> tower::Service<http::Request<ReqBody>>
    for NewPayloadWithWitnessService<S, H>
where
    S: tower::Service<
            http::Request<ReqBody>,
            Response = http::Response<http_body_util::Full<bytes::Bytes>>,
            Error = tower::BoxError,
        > + Clone
        + Send
        + 'static,
    S::Future: Send,
    H: NewPayloadWithWitnessHandler,
    ReqBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    ReqBody::Error: std::fmt::Display,
{
    type Response = http::Response<http_body_util::Full<bytes::Bytes>>;
    type Error = tower::BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        // Check if this is a request to our REST endpoint.
        if req.uri().path() == NEW_PAYLOAD_WITH_WITNESS_PATH {
            // Only POST is allowed.
            if req.method() != Method::POST {
                return Box::pin(async move {
                    Ok(json_error_response(
                        StatusCode::METHOD_NOT_ALLOWED,
                        -32600,
                        "Only POST method is allowed",
                    ))
                });
            }

            // Check Content-Type
            let content_type = req
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !content_type.starts_with("application/json") {
                return Box::pin(async move {
                    Ok(json_error_response(
                        StatusCode::BAD_REQUEST,
                        -32700,
                        "Content-Type must be application/json",
                    ))
                });
            }

            let handler = Arc::clone(&self.handler);
            return Box::pin(async move {
                // Read the full body
                let body_bytes = match read_body(req.into_body()).await {
                    Ok(b) => b,
                    Err(e) => {
                        warn!(target: "rpc::engine::rest", %e, "Failed to read request body");
                        return Ok(json_error_response(
                            StatusCode::BAD_REQUEST,
                            -32700,
                            format!("Failed to read request body: {e}"),
                        ));
                    }
                };

                handle_new_payload_with_witness(handler, body_bytes).await
            });
        }

        // Not our route — pass through to the inner JSON-RPC service.
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(req).await })
    }
}

/// Reads the full body from an HTTP request.
async fn read_body<B>(body: B) -> Result<bytes::Bytes, String>
where
    B: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: std::fmt::Display,
{
    use http_body_util::BodyExt;
    let collected = body.collect().await.map_err(|e| e.to_string())?;
    Ok(collected.to_bytes())
}

/// Core handler for the REST endpoint.
///
/// Parses the JSON body, calls `new_payload_v5`, and returns an SSZ response.
async fn handle_new_payload_with_witness<H: NewPayloadWithWitnessHandler>(
    handler: Arc<H>,
    body: bytes::Bytes,
) -> Result<
    http::Response<http_body_util::Full<bytes::Bytes>>,
    tower::BoxError,
> {
    trace!(target: "rpc::engine::rest", "Serving POST /new-payload-with-witness");

    // Parse the JSON body: a JSON array of exactly 4 elements.
    let params: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return Ok(json_error_response(
                StatusCode::BAD_REQUEST,
                -32700,
                format!("Parse error: {e}"),
            ));
        }
    };

    let arr = match params.as_array() {
        Some(a) => a,
        None => {
            return Ok(json_error_response(
                StatusCode::BAD_REQUEST,
                -32700,
                "Request body must be a JSON array",
            ));
        }
    };

    if arr.len() != 4 {
        return Ok(json_error_response(
            StatusCode::BAD_REQUEST,
            -32602,
            format!("Expected exactly 4 parameters, got {}", arr.len()),
        ));
    }

    // Parse individual parameters.
    let execution_payload: ExecutionPayloadV4 = match serde_json::from_value(arr[0].clone()) {
        Ok(p) => p,
        Err(e) => {
            return Ok(json_error_response(
                StatusCode::BAD_REQUEST,
                -32602,
                format!("Invalid executionPayload: {e}"),
            ));
        }
    };

    let versioned_hashes: Vec<B256> = match serde_json::from_value(arr[1].clone()) {
        Ok(h) => h,
        Err(e) => {
            return Ok(json_error_response(
                StatusCode::BAD_REQUEST,
                -32602,
                format!("Invalid expectedBlobVersionedHashes: {e}"),
            ));
        }
    };

    let parent_beacon_block_root: B256 = match serde_json::from_value(arr[2].clone()) {
        Ok(r) => r,
        Err(e) => {
            return Ok(json_error_response(
                StatusCode::BAD_REQUEST,
                -32602,
                format!("Invalid parentBeaconBlockRoot: {e}"),
            ));
        }
    };

    let execution_requests: RequestsOrHash = match serde_json::from_value(arr[3].clone()) {
        Ok(r) => r,
        Err(e) => {
            return Ok(json_error_response(
                StatusCode::BAD_REQUEST,
                -32602,
                format!("Invalid executionRequests: {e}"),
            ));
        }
    };

    // Check if execution requests hash is acceptable.
    if execution_requests.is_hash() && !handler.accept_execution_requests_hash() {
        return Ok(json_error_response(
            StatusCode::BAD_REQUEST,
            -38003,
            "requests hash cannot be accepted without --engine.accept-execution-requests-hash flag",
        ));
    }

    // Construct ExecutionData (same as engine_newPayloadV5).
    let payload = ExecutionData {
        payload: execution_payload.into(),
        sidecar: ExecutionPayloadSidecar::v4(
            CancunPayloadFields { versioned_hashes, parent_beacon_block_root },
            PraguePayloadFields { requests: execution_requests },
        ),
    };

    // Call newPayloadV5 logic.
    let status = match handler.new_payload_v5(payload).await {
        Ok(s) => s,
        Err(e) => {
            // Map EngineApiError to appropriate HTTP error responses.
            return Ok(map_engine_error_to_rest(e));
        }
    };

    debug!(
        target: "rpc::engine::rest",
        status = ?status.status,
        "POST /new-payload-with-witness result"
    );

    // TODO: When the status is VALID, generate the execution witness via re-execution.
    // For now, we return None for the witness field. A follow-up will add witness generation
    // by integrating with the block re-execution infrastructure (similar to debug_executionWitness).
    let witness: Option<&ExecutionWitness> = None;

    // SSZ-encode the response.
    let ssz_bytes = encode_new_payload_with_witness_response(&status, witness);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(http_body_util::Full::new(bytes::Bytes::from(ssz_bytes)))
        .expect("valid response"))
}

/// Maps an [`EngineApiError`] to an HTTP REST error response.
fn map_engine_error_to_rest(
    error: EngineApiError,
) -> http::Response<http_body_util::Full<bytes::Bytes>> {
    match &error {
        EngineApiError::EngineObjectValidationError(
            EngineObjectValidationError::UnsupportedFork,
        ) => json_error_response(
            StatusCode::BAD_REQUEST,
            -38005,
            error.to_string(),
        ),
        EngineApiError::EngineObjectValidationError(_) => json_error_response(
            StatusCode::BAD_REQUEST,
            -32602,
            error.to_string(),
        ),
        EngineApiError::UnexpectedRequestsHash => json_error_response(
            StatusCode::BAD_REQUEST,
            -38003,
            error.to_string(),
        ),
        _ => {
            // Internal errors
            json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                -32603,
                error.to_string(),
            )
        }
    }
}
