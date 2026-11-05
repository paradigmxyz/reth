use crate::{
	client::{
		BatchMessage, Error, FrontToBack, MiddlewareBatchResponse, MiddlewareMethodResponse, MiddlewareNotifResponse,
		RequestMessage, SubscriptionMessage, SubscriptionResponse,
	},
	middleware::{Batch, IsBatch, IsSubscription, Notification, Request, RpcServiceT},
};

use jsonrpsee_types::{Response, ResponsePayload};
use tokio::sync::{mpsc, oneshot};

impl From<mpsc::error::SendError<FrontToBack>> for Error {
	fn from(_: mpsc::error::SendError<FrontToBack>) -> Self {
		Error::ServiceDisconnect
	}
}

impl From<oneshot::error::RecvError> for Error {
	fn from(_: oneshot::error::RecvError) -> Self {
		Error::ServiceDisconnect
	}
}

/// RpcService implementation for the async client.
#[derive(Debug, Clone)]
pub struct RpcService(mpsc::Sender<FrontToBack>);

impl RpcService {
	// This is a private interface but we need to expose it for the async client
	// to be able to create the service.
	#[allow(private_interfaces)]
	pub(crate) fn new(tx: mpsc::Sender<FrontToBack>) -> Self {
		Self(tx)
	}
}

impl RpcServiceT for RpcService {
	type MethodResponse = Result<MiddlewareMethodResponse, Error>;
	type BatchResponse = Result<MiddlewareBatchResponse, Error>;
	type NotificationResponse = Result<MiddlewareNotifResponse, Error>;

	fn call<'a>(&self, request: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
		let tx = self.0.clone();

		async move {
			let raw = serde_json::to_string(&request)?;

			match request.extensions.get::<IsSubscription>() {
				Some(sub) => {
					let (send_back_tx, send_back_rx) = tokio::sync::oneshot::channel();

					tx.clone()
						.send(FrontToBack::Subscribe(SubscriptionMessage {
							raw,
							subscribe_id: sub.sub_req_id(),
							unsubscribe_id: sub.unsub_req_id(),
							unsubscribe_method: sub.unsubscribe_method().to_owned(),
							send_back: send_back_tx,
						}))
						.await?;

					let (subscribe_rx, sub_id) = send_back_rx.await??;

					let rp = serde_json::value::to_raw_value(&sub_id)?;

					Ok(MiddlewareMethodResponse::subscription_response(
						Response::new(ResponsePayload::success(rp), request.id.clone().into_owned()).into(),
						SubscriptionResponse { sub_id, stream: subscribe_rx },
					))
				}
				None => {
					let (send_back_tx, send_back_rx) = oneshot::channel();

					tx.send(FrontToBack::Request(RequestMessage {
						raw,
						send_back: Some(send_back_tx),
						id: request.id.clone().into_owned(),
					}))
					.await?;
					let mut rp = send_back_rx.await??;

					rp.0.extensions = request.extensions;

					Ok(MiddlewareMethodResponse::response(rp))
				}
			}
		}
	}

	fn batch<'a>(&self, mut batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		let tx = self.0.clone();

		async move {
			let (send_back_tx, send_back_rx) = oneshot::channel();

			let raw = serde_json::to_string(&batch)?;
			let id_range = batch
				.extensions()
				.get::<IsBatch>()
				.map(|b| b.id_range.clone())
				.expect("Batch ID range must be set in extensions");

			tx.send(FrontToBack::Batch(BatchMessage { raw, ids: id_range, send_back: send_back_tx })).await?;
			let json = send_back_rx.await??;

			Ok(json)
		}
	}

	fn notification<'a>(&self, n: Notification<'a>) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
		let tx = self.0.clone();

		async move {
			let raw = serde_json::to_string(&n)?;
			tx.send(FrontToBack::Notification(raw)).await?;
			Ok(MiddlewareNotifResponse::from(n.extensions))
		}
	}
}
