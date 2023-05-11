//! IPC request handling adapted from [`jsonrpsee`] http request handling
use futures::{stream::FuturesOrdered, StreamExt};
use jsonrpsee::{
    core::{
        server::helpers::{prepare_error, BatchResponseBuilder, MethodResponse},
        tracing::{rx_log_from_json, tx_log_from_str},
        JsonRawValue,
    },
    helpers::batch_response_error,
    server::{
        logger,
        logger::{Logger, TransportProtocol},
    },
    types::{error::ErrorCode, ErrorObject, Id, InvalidRequest, Notification, Params, Request},
    MethodCallback, Methods,
};
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::either::Either;
use tracing::instrument;

type Notif<'a> = Notification<'a, Option<&'a JsonRawValue>>;

#[derive(Debug, Clone)]
pub(crate) struct Batch<'a, L: Logger> {
    data: Vec<u8>,
    call: CallData<'a, L>,
}

#[derive(Debug, Clone)]
pub(crate) struct CallData<'a, L: Logger> {
    conn_id: usize,
    logger: &'a L,
    methods: &'a Methods,
    max_response_body_size: u32,
    max_log_length: u32,
    request_start: L::Instant,
}

// Batch responses must be sent back as a single message so we read the results from each
// request in the batch and read the results off of a new channel, `rx_batch`, and then send the
// complete batch response back to the client over `tx`.
#[instrument(name = "batch", skip(b), level = "TRACE")]
pub(crate) async fn process_batch_request<L>(b: Batch<'_, L>) -> String
where
    L: Logger,
{
    let Batch { data, call } = b;

    if let Ok(batch) = serde_json::from_slice::<Vec<&JsonRawValue>>(&data) {
        let mut got_notif = false;
        let mut batch_response =
            BatchResponseBuilder::new_with_limit(call.max_response_body_size as usize);

        let mut pending_calls: FuturesOrdered<_> = batch
            .into_iter()
            .filter_map(|v| {
                if let Ok(req) = serde_json::from_str::<Request<'_>>(v.get()) {
                    Some(Either::Right(execute_call(req, call.clone())))
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
                return too_large
            }
        }

        if got_notif && batch_response.is_empty() {
            String::new()
        } else {
            batch_response.finish()
        }
    } else {
        batch_response_error(Id::Null, ErrorObject::from(ErrorCode::ParseError))
    }
}

pub(crate) async fn process_single_request<L: Logger>(
    data: Vec<u8>,
    call: CallData<'_, L>,
) -> MethodResponse {
    if let Ok(req) = serde_json::from_slice::<Request<'_>>(&data) {
        execute_call_with_tracing(req, call).await
    } else if let Ok(notif) = serde_json::from_slice::<Notif<'_>>(&data) {
        execute_notification(notif, call.max_log_length)
    } else {
        let (id, code) = prepare_error(&data);
        MethodResponse::error(id, ErrorObject::from(code))
    }
}

#[instrument(name = "method_call", fields(method = req.method.as_ref()), skip(call, req), level = "TRACE")]
pub(crate) async fn execute_call_with_tracing<'a, L: Logger>(
    req: Request<'a>,
    call: CallData<'_, L>,
) -> MethodResponse {
    execute_call(req, call).await
}

pub(crate) async fn execute_call<L: Logger>(
    req: Request<'_>,
    call: CallData<'_, L>,
) -> MethodResponse {
    let CallData {
        methods,
        logger,
        max_response_body_size,
        max_log_length,
        conn_id,
        request_start,
    } = call;

    rx_log_from_json(&req, call.max_log_length);

    let params = Params::new(req.params.map(|params| params.get()));
    let name = &req.method;
    let id = req.id;

    let response = match methods.method_with_name(name) {
        None => {
            logger.on_call(
                name,
                params.clone(),
                logger::MethodKind::Unknown,
                TransportProtocol::Http,
            );
            MethodResponse::error(id, ErrorObject::from(ErrorCode::MethodNotFound))
        }
        Some((name, method)) => match method {
            MethodCallback::Sync(callback) => {
                logger.on_call(
                    name,
                    params.clone(),
                    logger::MethodKind::MethodCall,
                    TransportProtocol::Http,
                );
                (callback)(id, params, max_response_body_size as usize)
            }
            MethodCallback::Async(callback) => {
                logger.on_call(
                    name,
                    params.clone(),
                    logger::MethodKind::MethodCall,
                    TransportProtocol::Http,
                );
                let id = id.into_owned();
                let params = params.into_owned();
                (callback)(id, params, conn_id, max_response_body_size as usize).await
            }
            MethodCallback::Subscription(_) | MethodCallback::Unsubscription(_) => {
                logger.on_call(
                    name,
                    params.clone(),
                    logger::MethodKind::Unknown,
                    TransportProtocol::Http,
                );
                tracing::error!("Subscriptions not supported on HTTP");
                MethodResponse::error(id, ErrorObject::from(ErrorCode::InternalError))
            }
        },
    };

    tx_log_from_str(&response.result, max_log_length);
    logger.on_result(name, response.success, request_start, TransportProtocol::Http);
    response
}

#[instrument(name = "notification", fields(method = notif.method.as_ref()), skip(notif, max_log_length), level = "TRACE")]
fn execute_notification(notif: Notif<'_>, max_log_length: u32) -> MethodResponse {
    rx_log_from_json(&notif, max_log_length);
    let response = MethodResponse { result: String::new(), success: true };
    tx_log_from_str(&response.result, max_log_length);
    response
}

#[allow(unused)]
pub(crate) struct HandleRequest<L: Logger> {
    pub(crate) methods: Methods,
    pub(crate) max_request_body_size: u32,
    pub(crate) max_response_body_size: u32,
    pub(crate) max_log_length: u32,
    pub(crate) batch_requests_supported: bool,
    pub(crate) logger: L,
    pub(crate) conn: Arc<OwnedSemaphorePermit>,
}

pub(crate) async fn handle_request<L: Logger>(request: String, input: HandleRequest<L>) -> String {
    let HandleRequest { methods, max_response_body_size, max_log_length, logger, conn, .. } = input;

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

    let request_start = logger.on_request(TransportProtocol::Http);

    let call = CallData {
        conn_id: 0,
        logger: &logger,
        methods: &methods,
        max_response_body_size,
        max_log_length,
        request_start,
    };
    // Single request or notification
    let res = if matches!(request_kind, Kind::Single) {
        let response = process_single_request(request.into_bytes(), call).await;
        response.result
    } else {
        process_batch_request(Batch { data: request.into_bytes(), call }).await
    };

    drop(conn);

    res
}
