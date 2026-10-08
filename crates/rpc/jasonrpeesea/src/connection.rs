use crate::{
    error::exceeded_limit,
    request::{parse_message, split_batch, Message},
    response::batch_json,
    subscription::Connection,
    Id, MethodResponse, Methods, RpcService, RpcServiceBuilder, RpcServiceT, ServerConfig,
    StopHandle, OVERSIZED_REQUEST_CODE, OVERSIZED_REQUEST_MSG,
};
use bytes::Bytes;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::sync::Arc;
use tokio::{sync::mpsc, task::JoinSet};
use tower::Layer;

/// Serves JSON-RPC messages read from `reader` and writes responses and notifications to
/// `writer`, until either closes or the server stops.
///
/// Calls run concurrently and are aborted when the connection ends.
pub async fn serve_connection<R, W, L>(
    mut reader: R,
    writer: W,
    methods: Methods,
    rpc_middleware: &RpcServiceBuilder<L>,
    config: &ServerConfig,
    stop: StopHandle,
) where
    R: Stream<Item = Bytes> + Unpin,
    W: Sink<String> + Unpin,
    L: Layer<RpcService>,
    L::Service: RpcServiceT + 'static,
{
    let (tx, mut rx) = mpsc::channel::<String>(config.message_buffer_capacity.max(1) as usize);
    let conn = Arc::new(Connection::new(
        tx.clone(),
        config.max_subscriptions_per_connection,
        config.id_provider.clone(),
    ));
    let max_request_size = config.max_request_body_size as usize;
    let max_response_size = config.max_response_body_size as usize;
    let service =
        Arc::new(rpc_middleware.service(RpcService::new(methods, max_response_size, Some(conn))));
    let mut calls = JoinSet::new();

    let read = async {
        while let Some(msg) = reader.next().await {
            while calls.try_join_next().is_some() {}
            // Reserve the response slot first to apply backpressure on the reader.
            let Ok(permit) = tx.clone().reserve_owned().await else { break };
            if msg.len() > max_request_size {
                let err =
                    exceeded_limit(OVERSIZED_REQUEST_CODE, OVERSIZED_REQUEST_MSG, max_request_size);
                permit.send(MethodResponse::error(Id::Null, err).into_json());
                continue
            }
            let service = service.clone();
            calls.spawn(async move {
                if let Some(json) = handle_message(&*service, msg, max_response_size).await {
                    permit.send(json);
                }
            });
        }
    };

    let mut writer = writer;
    let write = async {
        while let Some(msg) = rx.recv().await {
            if writer.feed(msg).await.is_err() {
                return
            }
            while let Ok(msg) = rx.try_recv() {
                if writer.feed(msg).await.is_err() {
                    return
                }
            }
            if writer.flush().await.is_err() {
                return
            }
        }
    };

    tokio::select! {
        () = read => {}
        () = write => {}
        () = stop.shutdown() => {}
    }
    drop(calls);
    let _ = writer.close().await;
}

/// Handles a single message or batch, returning the response to send, if any.
pub(crate) async fn handle_message<S: RpcServiceT>(
    service: &S,
    msg: Bytes,
    max_response_size: usize,
) -> Option<String> {
    let entries = match split_batch(&msg) {
        None => {
            return match parse_message(&msg) {
                Message::Call(req) => {
                    let response = service.call(req).await;
                    (!response.is_subscription()).then(|| response.into_json())
                }
                Message::Notification => None,
                Message::Invalid(id, code) => Some(MethodResponse::error(id, code).into_json()),
            }
        }
        Some(Ok(entries)) => entries,
        Some(Err(code)) => return Some(MethodResponse::error(Id::Null, code).into_json()),
    };

    // Invalid entries are answered in place, calls are filled in after the batch completes.
    let mut responses = Vec::with_capacity(entries.len());
    let mut reqs = Vec::with_capacity(entries.len());
    for entry in &entries {
        match parse_message(entry) {
            Message::Call(req) => {
                reqs.push(req);
                responses.push(None);
            }
            Message::Notification => {}
            Message::Invalid(id, code) => responses.push(Some(MethodResponse::error(id, code))),
        }
    }
    drop(entries);
    let mut results =
        if reqs.is_empty() { Vec::new() } else { service.batch(reqs).await }.into_iter();
    let responses =
        responses.into_iter().filter_map(|response| response.or_else(|| results.next()));
    batch_json(responses, max_response_size)
}
