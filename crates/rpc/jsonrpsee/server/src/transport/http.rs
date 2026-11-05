use crate::{
	BatchRequestConfig, ConnectionState, HttpRequest, HttpResponse, LOG_TARGET,
	middleware::rpc::{RpcService, RpcServiceCfg},
	server::{ServerConfig, handle_rpc_call},
};
use http::Method;
use hyper::body::{Body, Bytes};
use jsonrpsee_core::{
	BoxError,
	http_helpers::{HttpError, read_body},
	middleware::{RpcServiceBuilder, RpcServiceT},
	server::{MethodResponse, Methods},
};

/// Checks that content type of received request is valid for JSON-RPC.
pub fn content_type_is_json<T: Body>(request: &HttpRequest<T>) -> bool {
	is_json(request.headers().get(hyper::header::CONTENT_TYPE))
}

/// Returns true if the `content_type` header indicates a valid JSON message.
pub fn is_json(content_type: Option<&hyper::header::HeaderValue>) -> bool {
	content_type.and_then(|val| val.to_str().ok()).is_some_and(|content| {
		content.eq_ignore_ascii_case("application/json")
			|| content.eq_ignore_ascii_case("application/json; charset=utf-8")
			|| content.eq_ignore_ascii_case("application/json;charset=utf-8")
			|| content.eq_ignore_ascii_case("application/json-rpc")
			|| content.eq_ignore_ascii_case("application/json-rpc;charset=utf-8")
			|| content.eq_ignore_ascii_case("application/json-rpc; charset=utf-8")
	})
}

/// Make JSON-RPC HTTP call with a [`RpcServiceBuilder`]
///
/// Fails if the HTTP request was a malformed JSON-RPC request.
pub async fn call_with_service_builder<L, B>(
	request: HttpRequest<B>,
	server_cfg: ServerConfig,
	conn: ConnectionState,
	methods: impl Into<Methods>,
	rpc_service: RpcServiceBuilder<L>,
) -> HttpResponse
where
	B: http_body::Body<Data = Bytes> + Send + 'static,
	B::Data: Send,
	B::Error: Into<BoxError>,
	L: tower::Layer<RpcService>,
	<L as tower::Layer<RpcService>>::Service: RpcServiceT<
			MethodResponse = MethodResponse,
			BatchResponse = MethodResponse,
			NotificationResponse = MethodResponse,
		> + Send,
{
	let ServerConfig { max_response_body_size, batch_requests_config, max_request_body_size, .. } = server_cfg;

	let rpc_service = rpc_service.service(RpcService::new(
		methods.into(),
		max_response_body_size as usize,
		conn.conn_id.into(),
		RpcServiceCfg::OnlyCalls,
	));

	let rp = call_with_service(request, batch_requests_config, max_request_body_size, rpc_service).await;

	drop(conn);

	rp
}

/// Make JSON-RPC HTTP call with a service [`RpcServiceT`]
///
/// Fails if the HTTP request was a malformed JSON-RPC request.
pub async fn call_with_service<S, B>(
	request: HttpRequest<B>,
	batch_config: BatchRequestConfig,
	max_request_size: u32,
	rpc_service: S,
) -> HttpResponse
where
	B: http_body::Body<Data = Bytes> + Send + 'static,
	B::Data: Send,
	B::Error: Into<BoxError>,
	S: RpcServiceT<
			MethodResponse = MethodResponse,
			BatchResponse = MethodResponse,
			NotificationResponse = MethodResponse,
		> + Send,
{
	// Only the `POST` method is allowed.
	match *request.method() {
		Method::POST if content_type_is_json(&request) => {
			let (parts, body) = request.into_parts();

			let (body, is_single) = match read_body(&parts.headers, body, max_request_size).await {
				Ok(r) => r,
				Err(HttpError::TooLarge) => return response::too_large(max_request_size),
				Err(HttpError::Malformed) => return response::malformed(),
				Err(HttpError::Stream(e)) => {
					tracing::warn!(target: LOG_TARGET, "Internal error reading request body: {}", e);
					return response::internal_error();
				}
			};

			let rp = handle_rpc_call(&body, is_single, batch_config, &rpc_service, parts.extensions).await;

			// If the response is empty it means that it was a notification or empty batch.
			// For HTTP these are just ACK:ed with a empty body.
			response::from_method_response(rp)
		}
		// Error scenarios:
		Method::POST => response::unsupported_content_type(),
		_ => response::method_not_allowed(),
	}
}

/// HTTP response helpers.
pub mod response {
	use jsonrpsee_core::server::MethodResponse;
	use jsonrpsee_types::error::{ErrorCode, reject_too_big_request};
	use jsonrpsee_types::{ErrorObject, ErrorObjectOwned, Id, Response, ResponsePayload};

	use crate::{HttpBody, HttpResponse};

	const JSON: &str = "application/json; charset=utf-8";
	const TEXT: &str = "text/plain";

	/// Create a response for json internal error.
	pub fn internal_error() -> HttpResponse {
		let err = ResponsePayload::<()>::error(ErrorObjectOwned::from(ErrorCode::InternalError));
		let rp = Response::new(err, Id::Null);
		let error = serde_json::to_string(&rp).expect("built from known-good data; qed");

		from_template(hyper::StatusCode::INTERNAL_SERVER_ERROR, error, JSON)
	}

	/// Create a json response for general errors returned by the called method.
	pub fn error_response(error: ErrorObject) -> HttpResponse {
		let error = serde_json::to_string(&error).expect("JSON serialization infallible; qed");
		from_template(hyper::StatusCode::INTERNAL_SERVER_ERROR, error, JSON)
	}

	/// Create a text/plain response for not allowed hosts.
	pub fn host_not_allowed() -> HttpResponse {
		from_template(hyper::StatusCode::FORBIDDEN, "Provided Host header is not whitelisted.\n", TEXT)
	}

	/// Create a text/plain response for disallowed method used.
	pub fn method_not_allowed() -> HttpResponse {
		from_template(
			hyper::StatusCode::METHOD_NOT_ALLOWED,
			"Used HTTP Method is not allowed. POST is required\n",
			TEXT,
		)
	}

	/// Create a json response for oversized requests (413)
	pub fn too_large(limit: u32) -> HttpResponse {
		let err = ResponsePayload::<()>::error(reject_too_big_request(limit));
		let rp = Response::new(err, Id::Null);
		let error = serde_json::to_string(&rp).expect("JSON serialization infallible; qed");

		from_template(hyper::StatusCode::PAYLOAD_TOO_LARGE, error, JSON)
	}

	/// Create a json response for empty or malformed requests (400)
	pub fn malformed() -> HttpResponse {
		let rp = Response::new(ResponsePayload::<()>::error(ErrorCode::ParseError), Id::Null);
		let error = serde_json::to_string(&rp).expect("JSON serialization infallible; qed");

		from_template(hyper::StatusCode::BAD_REQUEST, error, JSON)
	}

	/// Create a response body.
	pub(crate) fn from_template(
		status: hyper::StatusCode,
		body: impl Into<HttpBody>,
		content_type: &'static str,
	) -> HttpResponse {
		HttpResponse::builder()
			.status(status)
			.header("content-type", hyper::header::HeaderValue::from_static(content_type))
			.body(body.into())
			// Parsing `StatusCode` and `HeaderValue` is infalliable but
			// parsing body content is not.
			.expect("Unable to parse response body for type conversion")
	}

	/// Create a valid JSON response.
	pub fn ok_response(body: impl Into<HttpBody>) -> HttpResponse {
		from_template(hyper::StatusCode::OK, body, JSON)
	}

	/// Create a response from a method response.
	///
	/// This will include the body and extensions from the method response.
	pub fn from_method_response(rp: MethodResponse) -> HttpResponse {
		let (body, _, extensions) = rp.into_parts();
		let mut rp = from_template(hyper::StatusCode::OK, String::from(Box::<str>::from(body)), JSON);
		rp.extensions_mut().extend(extensions);
		rp
	}

	/// Create a response for unsupported content type.
	pub fn unsupported_content_type() -> HttpResponse {
		from_template(
			hyper::StatusCode::UNSUPPORTED_MEDIA_TYPE,
			"Supplied content type is not allowed. Content-Type: application/json is required\n",
			TEXT,
		)
	}

	/// Create a response for when the server is busy and can't accept more requests.
	pub fn too_many_requests() -> HttpResponse {
		from_template(hyper::StatusCode::TOO_MANY_REQUESTS, "Too many connections. Please try again later.", TEXT)
	}

	/// Create a response for when the server denied the request.
	pub fn denied() -> HttpResponse {
		from_template(hyper::StatusCode::FORBIDDEN, HttpBody::default(), TEXT)
	}
}
