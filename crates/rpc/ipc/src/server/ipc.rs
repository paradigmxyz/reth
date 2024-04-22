//! IPC request handling adapted from [`jsonrpsee`] http request handling

use futures::{stream::FuturesOrdered, StreamExt};
use jsonrpsee::{
    batch_response_error,
    core::{
        server::helpers::prepare_error,
        tracing::server::{rx_log_from_json, tx_log_from_str},
        JsonRawValue,
    },
    server::{middleware::rpc::RpcServiceT, IdProvider},
    types::{
        error::{reject_too_many_subscriptions, ErrorCode},
        ErrorObject, Id, InvalidRequest, Notification, Params, Request,
    },
    BatchResponseBuilder, BoundedSubscriptions, CallOrSubscription, MethodCallback, MethodResponse,
    MethodSink, Methods, ResponsePayload, SubscriptionState,
};
use std::{sync::Arc, time::Instant};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::either::Either;
use tracing::instrument;

type Notif<'a> = Notification<'a, Option<&'a JsonRawValue>>;

#[derive(Debug, Clone)]
pub(crate) struct Batch<'a, S> {
    data: Vec<u8>,
    call: CallData<'a>,
    rpc_service: S,
}

// todo(abner) remove it
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct CallData<'a> {
    conn_id: usize,
    methods: &'a Methods,
    id_provider: &'a dyn IdProvider,
    sink: &'a MethodSink,
    max_response_body_size: u32,
    max_log_length: u32,
    request_start: Instant,
    bounded_subscriptions: BoundedSubscriptions,
}

// Batch responses must be sent back as a single message so we read the results from each
// request in the batch and read the results off of a new channel, `rx_batch`, and then send the
// complete batch response back to the client over `tx`.
#[instrument(name = "batch", skip(b), level = "TRACE")]
pub(crate) async fn process_batch_request<S>(b: Batch<'_, S>) -> Option<String>
where
    for<'a> S: RpcServiceT<'a> + Send,
{
    let Batch { data, call, rpc_service } = b;

    if let Ok(batch) = serde_json::from_slice::<Vec<&JsonRawValue>>(&data) {
        let mut got_notif = false;
        let mut batch_response =
            BatchResponseBuilder::new_with_limit(call.max_response_body_size as usize);

        let mut pending_calls: FuturesOrdered<_> = batch
            .into_iter()
            .filter_map(|v| {
                if let Ok(req) = serde_json::from_str::<Request<'_>>(v.get()) {
                    Some(Either::Right(rpc_service.call(req)))
                } else if let Ok(_notif) = serde_json::from_str::<Notif<'_>>(v.get()) {
                    // notifications should not be answered.
                    got_notif = true;
                    None
                } else {
                    // valid JSON but could be not parsable as `InvalidRequest`
                    let id = match serde_json::from_str::<InvalidRequest<'_>>(v.get()) {
                        Ok(err) => err.id,
                        Err(_) => Id::Null,
                    };

                    Some(Either::Left(async {
                        MethodResponse::error(id, ErrorObject::from(ErrorCode::InvalidRequest))
                    }))
                }
            })
            .collect();

        while let Some(response) = pending_calls.next().await {
            if let Err(too_large) = batch_response.append(&response) {
                return Some(too_large.to_result())
            }
        }

        if got_notif && batch_response.is_empty() {
            None
        } else {
            let batch_resp = batch_response.finish();
            Some(MethodResponse::from_batch(batch_resp).to_result())
        }
    } else {
        Some(batch_response_error(Id::Null, ErrorObject::from(ErrorCode::ParseError)))
    }
}

pub(crate) async fn process_single_request<S>(
    data: Vec<u8>,
    rpc_service: &S,
    call: CallData<'_>,
) -> Option<MethodResponse>
where
    for<'a> S: RpcServiceT<'a> + Send,
{
    if let Ok(req) = serde_json::from_slice::<Request<'_>>(&data) {
        Some(execute_call_with_tracing(req, rpc_service, call).await)
    } else if serde_json::from_slice::<Notif<'_>>(&data).is_ok() {
        None
    } else {
        let (id, code) = prepare_error(&data);
        Some(MethodResponse::error(id, ErrorObject::from(code)))
    }
}

// #[instrument(name = "method_call", fields(method = req.method.as_ref()), skip(call, req), level =
// "TRACE")]
pub(crate) async fn execute_call_with_tracing<'a, S>(
    req: Request<'a>,
    rpc_service: &S,
    _call: CallData<'_>,
) -> MethodResponse
where
    for<'b> S: RpcServiceT<'b> + Send,
{
    rpc_service.call(req).await
}

// todo(abner) remove it
#[allow(dead_code)]
pub(crate) async fn execute_call<S>(req: Request<'_>, call: CallData<'_>) -> CallOrSubscription
where
    for<'a> S: RpcServiceT<'a> + Send,
{
    let CallData {
        methods,
        max_response_body_size,
        max_log_length,
        conn_id,
        id_provider,
        sink,
        request_start,
        bounded_subscriptions,
    } = call;

    rx_log_from_json(&req, call.max_log_length);

    let params = Params::new(req.params.as_ref().map(|params| params.get()));
    let name = &req.method;
    let id = req.id;

    let response = match methods.method_with_name(name) {
        None => {
            let response = MethodResponse::error(id, ErrorObject::from(ErrorCode::MethodNotFound));
            CallOrSubscription::Call(response)
        }
        Some((_name, method)) => match method {
            MethodCallback::Sync(callback) => {
                let response = (callback)(id, params, max_response_body_size as usize);
                CallOrSubscription::Call(response)
            }
            MethodCallback::Async(callback) => {
                let id = id.into_owned();
                let params = params.into_owned();
                let response =
                    (callback)(id, params, conn_id, max_response_body_size as usize).await;
                CallOrSubscription::Call(response)
            }
            MethodCallback::AsyncWithDetails(_callback) => {
                unimplemented!()
            }
            MethodCallback::Subscription(callback) => {
                if let Some(p) = bounded_subscriptions.acquire() {
                    let conn_state =
                        SubscriptionState { conn_id, id_provider, subscription_permit: p };
                    let response = callback(id, params, sink.clone(), conn_state).await;
                    CallOrSubscription::Subscription(response)
                } else {
                    let response = MethodResponse::error(
                        id,
                        reject_too_many_subscriptions(bounded_subscriptions.max()),
                    );
                    CallOrSubscription::Call(response)
                }
            }
            MethodCallback::Unsubscription(callback) => {
                // Don't adhere to any resource or subscription limits; always let unsubscribing
                // happen!
                let result = callback(id, params, conn_id, max_response_body_size as usize);
                CallOrSubscription::Call(result)
            }
        },
    };

    tx_log_from_str(response.as_response().as_result(), max_log_length);
    let _ = request_start;
    response
}

#[instrument(name = "notification", fields(method = notif.method.as_ref()), skip(notif, max_log_length), level = "TRACE")]
fn execute_notification(notif: &Notif<'_>, max_log_length: u32) -> MethodResponse {
    rx_log_from_json(notif, max_log_length);
    let response =
        MethodResponse::response(Id::Null, ResponsePayload::success(String::new()), usize::MAX);
    tx_log_from_str(response.as_result(), max_log_length);
    response
}

#[allow(dead_code)]
pub(crate) struct HandleRequest {
    pub(crate) methods: Methods,
    pub(crate) max_request_body_size: u32,
    pub(crate) max_response_body_size: u32,
    pub(crate) max_log_length: u32,
    pub(crate) batch_requests_supported: bool,
    pub(crate) conn: Arc<OwnedSemaphorePermit>,
    pub(crate) bounded_subscriptions: BoundedSubscriptions,
    pub(crate) method_sink: MethodSink,
    pub(crate) id_provider: Arc<dyn IdProvider>,
}

pub(crate) async fn call_with_service<S>(
    request: String,
    rpc_service: S,
    input: HandleRequest,
) -> Option<String>
where
    for<'a> S: RpcServiceT<'a> + Send,
{
    let HandleRequest {
        methods,
        max_response_body_size,
        max_log_length,
        conn,
        bounded_subscriptions,
        method_sink,
        id_provider,
        ..
    } = input;

    enum Kind {
        Single,
        Batch,
    }

    let request_kind = request
        .chars()
        .find_map(|c| match c {
            '{' => Some(Kind::Single),
            '[' => Some(Kind::Batch),
            _ => None,
        })
        .unwrap_or(Kind::Single);

    let call = CallData {
        conn_id: 0,
        methods: &methods,
        id_provider: &*id_provider,
        sink: &method_sink,
        max_response_body_size,
        max_log_length,
        request_start: Instant::now(),
        bounded_subscriptions,
    };

    // Single request or notification
    let res = if matches!(request_kind, Kind::Single) {
        let response = process_single_request(request.into_bytes(), &rpc_service, call).await;
        match response {
            Some(response) if response.is_method_call() => Some(response.to_result()),
            _ => {
                // subscription responses are sent directly over the sink, return a response here
                // would lead to duplicate responses for the subscription response
                None
            }
        }
    } else {
        process_batch_request(Batch { data: request.into_bytes(), rpc_service, call }).await
    };

    drop(conn);

    res
}
