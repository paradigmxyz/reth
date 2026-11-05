// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Middleware that proxies HTTP GET requests at a specified URI to an internal RPC method call.

use crate::transport::http;
use crate::{HttpBody, HttpRequest, HttpResponse};
use futures_util::{FutureExt, TryFutureExt};
use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE};
use hyper::http::HeaderValue;
use hyper::{Method, StatusCode, Uri};
use jsonrpsee_core::BoxError;
use jsonrpsee_types::{ErrorCode, ErrorObject, Id, Request};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// Error that occur if the specified path doesn't start with `/<path>`
#[derive(Debug, thiserror::Error)]
pub enum ProxyGetRequestError {
	/// Duplicated path.
	#[error("ProxyGetRequestLayer path must be unique, got duplicated `{0}`")]
	DuplicatedPath(String),
	/// Invalid path.
	#[error("ProxyGetRequestLayer path must start with `/`, got `{0}`")]
	InvalidPath(String),
}

/// Layer that applies [`ProxyGetRequest`] which proxies the `GET /path` requests to
/// specific RPC method calls and that strips the response.
///
/// See [`ProxyGetRequest`] for more details.
#[derive(Debug, Clone)]
pub struct ProxyGetRequestLayer {
	// path => method mapping
	methods: Arc<HashMap<String, String>>,
}

impl ProxyGetRequestLayer {
	/// Creates a new [`ProxyGetRequestLayer`].
	///
	/// The request `GET /path` is redirected to the provided method.
	/// Fails if the path does not start with `/`.
	pub fn new<P, M>(pairs: impl IntoIterator<Item = (P, M)>) -> Result<Self, ProxyGetRequestError>
	where
		P: Into<String>,
		M: Into<String>,
	{
		let mut methods = HashMap::new();

		for (path, method) in pairs {
			let path = path.into();
			let method = method.into();

			if !path.starts_with('/') {
				return Err(ProxyGetRequestError::InvalidPath(path));
			}

			if let Some(path) = methods.insert(path, method) {
				return Err(ProxyGetRequestError::DuplicatedPath(path));
			}
		}

		Ok(Self { methods: Arc::new(methods) })
	}
}

impl<S> Layer<S> for ProxyGetRequestLayer {
	type Service = ProxyGetRequest<S>;

	fn layer(&self, inner: S) -> Self::Service {
		ProxyGetRequest { inner, methods: self.methods.clone() }
	}
}

/// Proxy `GET /path` requests to the specified RPC method calls.
///
/// # Request
///
/// The `GET /path` requests are modified into valid `POST` requests for
/// calling the RPC method. This middleware adds appropriate headers to the
/// request, and completely modifies the request `BODY`.
///
/// # Response
///
/// The response of the RPC method is stripped down to contain only the method's
/// response, removing any RPC 2.0 spec logic regarding the response' body.
#[derive(Debug, Clone)]
pub struct ProxyGetRequest<S> {
	inner: S,
	// path => method mapping
	methods: Arc<HashMap<String, String>>,
}

impl<S, B> Service<HttpRequest<B>> for ProxyGetRequest<S>
where
	S: Service<HttpRequest, Response = HttpResponse>,
	S::Response: 'static,
	S::Error: Into<BoxError> + 'static,
	S::Future: Send + 'static,
	B: http_body::Body<Data = Bytes> + Send + 'static,
	B::Data: Send,
	B::Error: Into<BoxError>,
{
	type Response = S::Response;
	type Error = BoxError;
	type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

	#[inline]
	fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.inner.poll_ready(cx).map_err(Into::into)
	}

	fn call(&mut self, mut req: HttpRequest<B>) -> Self::Future {
		let path = req.uri().path();
		let method = self.methods.get(path);

		match (method, req.method()) {
			// Proxy the `GET /path` request to the appropriate method call.
			(Some(method), &Method::GET) => {
				// RPC methods are accessed with `POST`.
				*req.method_mut() = Method::POST;
				// Precautionary remove the URI path.
				*req.uri_mut() = if let Some(query) = req.uri().query() {
					Uri::from_str(&format!("/?{}", query)).expect("The query comes from a valid URI; qed")
				} else {
					Uri::from_static("/")
				};
				// Requests must have the following headers:
				req.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
				req.headers_mut().insert(ACCEPT, HeaderValue::from_static("application/json"));

				// Adjust the body to reflect the method call.
				let bytes =
					serde_json::to_vec(&Request::borrowed(method, None, Id::Number(0))).expect("Valid request; qed");
				let req = req.map(|_| HttpBody::from(bytes));

				// Call the inner service and get a future that resolves to the response.
				let fut = self.inner.call(req);

				async move {
					let res = fut.await.map_err(Into::into)?;

					let (parts, body) = res.into_parts();
					let mut body = http_body_util::BodyStream::new(body);
					let mut bytes = Vec::new();

					while let Some(frame) = body.frame().await {
						let data = frame?.into_data().map_err(|e| format!("{e:?}"))?;
						bytes.extend(data);
					}

					#[derive(serde::Deserialize)]
					struct SuccessResponse<'a> {
						#[serde(borrow)]
						result: &'a serde_json::value::RawValue,
					}

					let mut response = if let Ok(payload) = serde_json::from_slice::<SuccessResponse>(&bytes) {
						http::response::ok_response(payload.result.to_string())
					} else {
						internal_proxy_error(&bytes)
					};

					response.extensions_mut().extend(parts.extensions);

					Ok(response)
				}
				.boxed()
			}
			// Call the inner service and get a future that resolves to the response.
			_ => {
				let req = req.map(HttpBody::new);
				self.inner.call(req).map_err(Into::into).boxed()
			}
		}
	}
}

fn internal_proxy_error(bytes: &[u8]) -> HttpResponse {
	#[derive(serde::Deserialize)]
	struct ErrorResponse<'a> {
		#[serde(borrow)]
		error: ErrorObject<'a>,
	}

	let error = serde_json::from_slice::<ErrorResponse>(bytes)
		.map(|payload| payload.error)
		.unwrap_or_else(|_| ErrorObject::from(ErrorCode::InternalError));

	http::response::from_template(
		StatusCode::INTERNAL_SERVER_ERROR,
		serde_json::to_string(&error).expect("JSON serialization infallible; qed"),
		"application/json; charset=utf-8",
	)
}
