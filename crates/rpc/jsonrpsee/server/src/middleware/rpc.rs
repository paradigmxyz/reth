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

//! JSON-RPC service middleware.

pub use jsonrpsee_core::{middleware::*, server::MethodResponse};

use std::{panic::AssertUnwindSafe, sync::Arc};

use crate::ConnectionId;
use futures_util::FutureExt;
use jsonrpsee_core::{
    server::{
        BatchResponseBuilder, BoundedSubscriptions, MethodCallback, MethodSink, Methods,
        SubscriptionState,
    },
    traits::IdProvider,
};
use jsonrpsee_types::{
    error::{reject_too_many_subscriptions, ErrorCode},
    ErrorObject, Id,
};

/// JSON-RPC service middleware.
#[derive(Clone, Debug)]
pub struct RpcService {
    conn_id: ConnectionId,
    methods: Methods,
    max_response_body_size: usize,
    /// `None` if the transport can't send subscription notifications.
    subscriptions: Option<Subscriptions>,
}

/// State required to serve subscriptions on a connection.
#[derive(Clone, Debug)]
struct Subscriptions {
    bounded: BoundedSubscriptions,
    sink: MethodSink,
    id_provider: Arc<dyn IdProvider>,
}

impl RpcService {
    /// Creates a service for a connection that supports subscriptions.
    ///
    /// Subscription notifications are sent to `sink`, and at most `bounded_subscriptions` of them
    /// can be active at once.
    pub fn new(
        methods: Methods,
        max_response_body_size: usize,
        conn_id: ConnectionId,
        bounded_subscriptions: BoundedSubscriptions,
        sink: MethodSink,
        id_provider: Arc<dyn IdProvider>,
    ) -> Self {
        Self {
            methods,
            max_response_body_size,
            conn_id,
            subscriptions: Some(Subscriptions {
                bounded: bounded_subscriptions,
                sink,
                id_provider,
            }),
        }
    }

    /// Creates a service that answers subscription calls with an internal error.
    pub(crate) fn without_subscriptions(
        methods: Methods,
        max_response_body_size: usize,
        conn_id: ConnectionId,
    ) -> Self {
        Self { methods, max_response_body_size, conn_id, subscriptions: None }
    }
}

impl RpcServiceT for RpcService {
    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        let conn_id = self.conn_id;
        let max_response_body_size = self.max_response_body_size;

        let Request { id, method, params, .. } = req;
        let params = jsonrpsee_types::Params::new(
            params.as_ref().map(|p| serde_json::value::RawValue::get(p)),
        );

        let Some(method) = self.methods.method(&method) else {
            return ResponseFuture::ready(MethodResponse::error(
                id,
                ErrorObject::from(ErrorCode::MethodNotFound),
            ));
        };

        match method {
            MethodCallback::Async(callback) => ResponseFuture::future((callback)(
                id.into_owned(),
                params.into_owned(),
                conn_id,
                max_response_body_size,
            )),
            MethodCallback::Sync(callback) => {
                ResponseFuture::ready((callback)(id, params, max_response_body_size))
            }
            MethodCallback::Subscription(callback) => {
                let Some(subscriptions) = &self.subscriptions else {
                    tracing::warn!("Subscriptions not supported");
                    return ResponseFuture::ready(MethodResponse::error(
                        id,
                        ErrorObject::from(ErrorCode::InternalError),
                    ));
                };

                let Some(subscription_permit) = subscriptions.bounded.acquire() else {
                    let max = subscriptions.bounded.max();
                    return ResponseFuture::ready(MethodResponse::error(
                        id,
                        reject_too_many_subscriptions(max),
                    ));
                };

                let conn_state = SubscriptionState {
                    conn_id,
                    id_provider: &*subscriptions.id_provider,
                    subscription_permit,
                };
                ResponseFuture::future((callback)(
                    id,
                    params,
                    subscriptions.sink.clone(),
                    conn_state,
                ))
            }
            MethodCallback::Unsubscription(callback) => {
                // Don't adhere to any resource or subscription limits; always let unsubscribing
                // happen!
                if self.subscriptions.is_none() {
                    tracing::warn!("Subscriptions not supported");
                    return ResponseFuture::ready(MethodResponse::error(
                        id,
                        ErrorObject::from(ErrorCode::InternalError),
                    ));
                }

                ResponseFuture::ready(callback(id, params, conn_id, max_response_body_size))
            }
        }
    }

    fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        let mut batch_rp = BatchResponseBuilder::new_with_limit(self.max_response_body_size);
        let service = self.clone();
        async move {
            // Notifications and subscription calls get no entry in the batch response.
            let mut omitted_entry = false;

            // Entries run one after another, so a batch occupies at most one call at a time.
            for batch_entry in batch.into_iter() {
                // Entries that complete without ever returning `Pending` (sync methods, unknown
                // methods, invalid entries) would otherwise run the whole batch in one poll.
                // This yields only once the task's budget is used up, so a long batch can't hold
                // a worker or ignore cancellation.
                tokio::task::coop::consume_budget().await;

                match batch_entry {
                    Ok(BatchEntry::Call(req)) => {
                        let rp = catch_call_panic(req.id(), || service.call(req)).await;
                        // Accepting or rejecting a subscription already sent its response through
                        // the subscription sink.
                        if rp.is_subscription() {
                            omitted_entry = true;
                            continue;
                        }
                        if let Err(err) = batch_rp.append(rp) {
                            return err;
                        }
                    }
                    Ok(BatchEntry::Notification(n)) => {
                        omitted_entry = true;
                        service.notification(n).await;
                    }
                    Err(err) => {
                        let (err, id) = err.into_parts();
                        let rp = MethodResponse::error(id, err);
                        if let Err(err) = batch_rp.append(rp) {
                            return err;
                        }
                    }
                }
            }

            // If every entry was omitted, there is nothing to send back.
            if batch_rp.is_empty() && omitted_entry {
                MethodResponse::notification()
            }
            // An empty batch is regarded as an invalid request here.
            else {
                batch_rp.finish()
            }
        }
    }

    fn notification<'a>(
        &self,
        _n: Notification<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a {
        // Notifications are not answered.
        std::future::ready(MethodResponse::notification())
    }
}

/// Answers a panicking call with an internal error, otherwise the panic would only surface as a
/// failed connection or call task and the client would never receive a response for `id`.
///
/// `call` builds the call's future inside the guard, because sync methods run while
/// [`RpcServiceT::call`] builds it, not when it is polled.
pub(crate) async fn catch_call_panic<F>(id: Id<'_>, call: impl FnOnce() -> F) -> MethodResponse
where
    F: Future<Output = MethodResponse>,
{
    AssertUnwindSafe(async move { call().await })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| MethodResponse::error(id, ErrorObject::from(ErrorCode::InternalError)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RpcModule;
    use jsonrpsee_types::Request;
    use std::{
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
        task::Poll,
    };

    #[tokio::test]
    async fn batch_of_ready_entries_yields() {
        const ENTRIES: usize = 4096;

        let calls = Arc::new(AtomicUsize::new(0));
        let mut module = RpcModule::new(calls.clone());
        module.register_method("tick", |_, calls| calls.fetch_add(1, Ordering::SeqCst)).unwrap();
        let service =
            RpcService::without_subscriptions(module.into(), usize::MAX, ConnectionId::default());

        let batch = (0..ENTRIES as u64)
            .map(|id| Ok(BatchEntry::Call(Request::borrowed("tick", None, Id::Number(id)))))
            .collect::<Vec<_>>();
        let mut response = pin!(service.batch(Batch::from(batch)));

        // Every entry is ready, so only the task budget can make the batch yield.
        let first_poll = std::future::poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx))).await;
        assert!(first_poll.is_pending());
        assert!(calls.load(Ordering::SeqCst) < ENTRIES);

        let expected = (0..ENTRIES)
            .map(|i| format!(r#"{{"jsonrpc":"2.0","id":{i},"result":{i}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(response.await.into_json(), format!("[{expected}]"));
        assert_eq!(calls.load(Ordering::SeqCst), ENTRIES);
    }
}
