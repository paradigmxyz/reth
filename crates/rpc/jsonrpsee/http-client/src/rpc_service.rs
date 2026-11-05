use std::sync::Arc;

use hyper::body::Bytes;
use jsonrpsee_core::{
	BoxError, JsonRawValue,
	client::{Error, MiddlewareBatchResponse, MiddlewareMethodResponse, MiddlewareNotifResponse},
	middleware::{Batch, Notification, Request, RpcServiceT},
};
use jsonrpsee_types::Response;
use tower::Service;

use crate::{
	HttpRequest, HttpResponse,
	transport::{Error as TransportError, HttpTransportClient},
};

/// An [`RpcServiceT`] compliant implementation for the HTTP client.
#[derive(Clone, Debug)]
pub struct RpcService<HttpMiddleware> {
	service: Arc<HttpTransportClient<HttpMiddleware>>,
}

impl<HttpMiddleware> RpcService<HttpMiddleware> {
	/// Convert an [`HttpTransportClient`] into an [`RpcService`].
	pub fn new(service: HttpTransportClient<HttpMiddleware>) -> Self {
		Self { service: Arc::new(service) }
	}
}

impl<B, HttpMiddleware> RpcServiceT for RpcService<HttpMiddleware>
where
	HttpMiddleware:
		Service<HttpRequest, Response = HttpResponse<B>, Error = TransportError> + Clone + Send + Sync + 'static,
	HttpMiddleware::Future: Send,
	B: http_body::Body<Data = Bytes> + Send + 'static,
	B::Data: Send,
	B::Error: Into<BoxError>,
{
	type BatchResponse = Result<MiddlewareBatchResponse, Error>;
	type MethodResponse = Result<MiddlewareMethodResponse, Error>;
	type NotificationResponse = Result<MiddlewareNotifResponse, Error>;

	fn call<'a>(&self, request: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
		let service = self.service.clone();

		async move {
			let raw = serde_json::to_string(&request)?;
			let bytes = service.send_and_read_body(raw).await.map_err(|e| Error::Transport(e.into()))?;
			let mut rp: Response<Box<JsonRawValue>> = serde_json::from_slice(&bytes)?;
			rp.extensions = request.extensions;

			Ok(MiddlewareMethodResponse::response(rp.into_owned().into()))
		}
	}

	fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		let service = self.service.clone();

		async move {
			let raw = serde_json::to_string(&batch)?;
			let bytes = service.send_and_read_body(raw).await.map_err(|e| Error::Transport(e.into()))?;
			let rp: Vec<_> = serde_json::from_slice::<Vec<Response<Box<JsonRawValue>>>>(&bytes)?
				.into_iter()
				.map(|r| r.into_owned().into())
				.collect();

			Ok(rp)
		}
	}

	fn notification<'a>(
		&self,
		notif: Notification<'a>,
	) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
		let service = self.service.clone();

		async move {
			let raw = serde_json::to_string(&notif)?;
			service.send(raw).await.map_err(|e| Error::Transport(e.into()))?;
			Ok(notif.extensions.into())
		}
	}
}
