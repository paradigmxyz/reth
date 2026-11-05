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

use crate::client::async_client::manager::{RequestManager, RequestStatus};
use crate::client::async_client::{LOG_TARGET, Notification};
use crate::client::{
	Error, RawResponseOwned, RequestMessage, TransportSenderT, TrySubscriptionSendError, subscription_channel,
};
use crate::params::ArrayParams;
use crate::traits::ToRpcParams;

use futures_timer::Delay;
use futures_util::future::{self, Either};
use http::Extensions;
use serde_json::value::RawValue;
use tokio::sync::oneshot;

use jsonrpsee_types::response::SubscriptionError;
use jsonrpsee_types::{
	ErrorObject, Id, InvalidRequestId, Request, Response, ResponseSuccess, SubscriptionId, SubscriptionResponse,
	TwoPointZero,
};
use std::borrow::Cow;
use std::ops::Range;

/// Attempts to process a batch response.
///
/// On success the result is sent to the frontend.
pub(crate) fn process_batch_response(
	manager: &mut RequestManager,
	rps: Vec<RawResponseOwned>,
	range: Range<u64>,
) -> Result<(), InvalidRequestId> {
	let mut responses = Vec::with_capacity(rps.len());

	let start_idx = range.start;

	let batch_state = match manager.complete_pending_batch(range.clone()) {
		Some(state) => state,
		None => {
			tracing::debug!(target: LOG_TARGET, "Received unknown batch response");
			return Err(InvalidRequestId::NotPendingRequest(format!("{:?}", range)));
		}
	};

	for _ in range {
		let err_obj = ErrorObject::borrowed(0, "", None);
		responses.push(Response::new(jsonrpsee_types::ResponsePayload::error(err_obj), Id::Null).into());
	}

	for rp in rps {
		let id = rp.id().try_parse_inner_as_number()?;
		let maybe_elem =
			id.checked_sub(start_idx).and_then(|p| p.try_into().ok()).and_then(|p: usize| responses.get_mut(p));

		if let Some(elem) = maybe_elem {
			*elem = rp;
		} else {
			return Err(InvalidRequestId::NotPendingRequest(rp.id().to_string()));
		}
	}

	let _ = batch_state.send_back.send(Ok(responses));
	Ok(())
}

/// Attempts to process a subscription response.
///
/// Returns `Some(sub_id)` if the subscription should be closed otherwise
/// `None` is returned.
pub(crate) fn process_subscription_response(
	manager: &mut RequestManager,
	response: SubscriptionResponse<Box<RawValue>>,
) -> Option<SubscriptionId<'static>> {
	let sub_id = response.params.subscription.into_owned();
	let request_id = match manager.get_request_id_by_subscription_id(&sub_id) {
		Some(request_id) => request_id,
		None => {
			tracing::debug!(target: LOG_TARGET, "Subscription {:?} is not active", sub_id);
			return None;
		}
	};

	match manager.as_subscription_mut(&request_id) {
		Some(send_back_sink) => match send_back_sink.send(response.params.result) {
			Ok(_) => None,
			Err(TrySubscriptionSendError::Closed) => Some(sub_id),
			Err(TrySubscriptionSendError::TooSlow(m)) => {
				tracing::debug!(target: LOG_TARGET, "Subscription {{method={}, sub_id={:?}}} couldn't keep up with server; failed to send {m}", response.method, sub_id);
				Some(sub_id)
			}
		},
		None => {
			tracing::debug!(target: LOG_TARGET, "Subscription {:?} is not active", sub_id);
			None
		}
	}
}

/// Attempts to close a subscription when a [`SubscriptionError`] is received.
///
/// If the notification is not found it's just logged as a warning and the connection
/// will continue.
///
/// It's possible that the user closed down the subscription before the actual close response is received
pub(crate) fn process_subscription_close_response(
	manager: &mut RequestManager,
	response: SubscriptionError<&RawValue>,
) {
	let sub_id = response.params.subscription.into_owned();
	match manager.get_request_id_by_subscription_id(&sub_id) {
		Some(request_id) => {
			manager.remove_subscription(request_id, sub_id).expect("Both request ID and sub ID in RequestManager; qed");
		}
		None => {
			tracing::debug!(target: LOG_TARGET, "The server tried to close an non-pending subscription: {:?}", sub_id);
		}
	}
}

/// Attempts to process an incoming notification
///
/// If the notification is not found it's just logged as a warning and the connection
/// will continue.
///
/// It's possible that user close down the subscription before this notification is received.
pub(crate) fn process_notification(manager: &mut RequestManager, notif: Notification) {
	match manager.as_notification_handler_mut(notif.method.to_string()) {
		// If the notification doesn't have params, we just send an empty JSON object to indicate that to the user.
		Some(send_back_sink) => match send_back_sink.send(notif.params.unwrap_or_default()) {
			Ok(()) => (),
			Err(TrySubscriptionSendError::Closed) => {
				let _ = manager.remove_notification_handler(&notif.method);
			}
			Err(TrySubscriptionSendError::TooSlow(m)) => {
				tracing::debug!(target: LOG_TARGET, "Notification `{}` couldn't keep up with server; failed to send {m}", notif.method);
				let _ = manager.remove_notification_handler(&notif.method);
			}
		},
		None => {
			tracing::debug!(target: LOG_TARGET, "Notification: {:?} not a registered method", notif.method);
		}
	}
}

/// Process a response from the server.
///
/// Returns `Ok(None)` if the response was successfully sent.
/// Returns `Ok(Some(_))` if the response got an error but could be handled.
/// Returns `Err(_)` if the response couldn't be handled.
pub(crate) fn process_single_response(
	manager: &mut RequestManager,
	response: RawResponseOwned,
	max_capacity_per_subscription: usize,
) -> Result<Option<RequestMessage>, InvalidRequestId> {
	let response_id = response.id().clone().into_owned();

	match manager.request_status(&response_id) {
		RequestStatus::PendingMethodCall => {
			let send_back_oneshot = match manager.complete_pending_call(response_id.clone()) {
				Some(Some(send)) => send,
				Some(None) => return Ok(None),
				None => return Err(InvalidRequestId::NotPendingRequest(response_id.to_string())),
			};

			let _ = send_back_oneshot.send(Ok(response));
			Ok(None)
		}
		RequestStatus::PendingSubscription => {
			let (unsub_id, send_back_oneshot, unsubscribe_method) = manager
				.complete_pending_subscription(response_id.clone())
				.ok_or(InvalidRequestId::NotPendingRequest(response_id.to_string()))?;

			let result = ResponseSuccess::try_from(response.into_inner());

			let json = match result {
				Ok(s) => s.result,
				Err(e) => {
					let _ = send_back_oneshot.send(Err(Error::Call(e)));
					return Ok(None);
				}
			};

			let sub_id = match serde_json::from_str::<SubscriptionId>(json.get()) {
				Ok(s) => s.into_owned(),
				Err(e) => {
					let _ = send_back_oneshot.send(Err(e.into()));
					return Ok(None);
				}
			};

			let (subscribe_tx, subscribe_rx) = subscription_channel(max_capacity_per_subscription);
			if manager
				.insert_subscription(response_id.clone(), unsub_id, sub_id.clone(), subscribe_tx, unsubscribe_method)
				.is_ok()
			{
				match send_back_oneshot.send(Ok((subscribe_rx, sub_id.clone()))) {
					Ok(_) => Ok(None),
					Err(_) => Ok(build_unsubscribe_message(manager, response_id, sub_id)),
				}
			} else {
				let _ = send_back_oneshot.send(Err(Error::InvalidSubscriptionId));
				Ok(None)
			}
		}

		RequestStatus::Subscription | RequestStatus::Invalid => {
			Err(InvalidRequestId::NotPendingRequest(response_id.to_string()))
		}
	}
}

/// Sends an unsubscribe to request to server to indicate
/// that the client is not interested in the subscription anymore.
//
// NOTE: we don't count this a concurrent request as it's part of a subscription.
pub(crate) async fn stop_subscription<S: TransportSenderT>(
	sender: &mut S,
	unsub: RequestMessage,
) -> Result<(), S::Error> {
	sender.send(unsub.raw).await?;
	Ok(())
}

/// Builds an unsubscription message.
pub(crate) fn build_unsubscribe_message(
	manager: &mut RequestManager,
	sub_req_id: Id<'static>,
	sub_id: SubscriptionId<'static>,
) -> Option<RequestMessage> {
	let (unsub_req_id, _, unsub, sub_id) = manager.unsubscribe(sub_req_id, sub_id)?;

	let mut params = ArrayParams::new();
	params.insert(sub_id).ok()?;
	let params = params.to_rpc_params().ok()?;

	let raw = serde_json::to_string(&Request {
		jsonrpc: TwoPointZero,
		id: unsub_req_id.clone(),
		method: unsub.into(),
		params: params.map(Cow::Owned),
		extensions: Extensions::new(),
	})
	.ok()?;
	Some(RequestMessage { raw, id: unsub_req_id, send_back: None })
}

/// Wait for a stream to complete within the given timeout.
pub(crate) async fn call_with_timeout<T>(
	timeout: std::time::Duration,
	rx: oneshot::Receiver<Result<T, Error>>,
) -> Result<Result<T, Error>, oneshot::error::RecvError> {
	match future::select(rx, Delay::new(timeout)).await {
		Either::Left((res, _)) => res,
		Either::Right((_, _)) => Ok(Err(Error::RequestTimeout)),
	}
}
