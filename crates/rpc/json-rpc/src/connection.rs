use crate::{
    reject_too_big_batch_request, reject_too_big_request,
    request::{parse_message, split_batch, Message},
    response::batch_json,
    subscription::Connection,
    BatchRequestConfig, ConnectionId, ErrorObject, Id, MethodResponse, RpcModule, RpcService,
    RpcServiceBuilder, RpcServiceT, ServerConfig, StopHandle, BATCHES_NOT_SUPPORTED_CODE,
    BATCHES_NOT_SUPPORTED_MSG,
};
use bytes::Bytes;
use futures_util::{
    future::{join, join_all},
    Sink, SinkExt, Stream, StreamExt,
};
use http::Extensions;
use std::sync::Arc;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tower::Layer;

/// Serves JSON-RPC messages read from `reader` and writes responses and notifications to
/// `writer`, until either closes or the server stops.
///
/// Calls run concurrently and are aborted when the connection ends. `extensions` are attached to
/// every request, together with a new [`ConnectionId`] unless they already contain one.
pub async fn serve_connection<R, W, L>(
    mut reader: R,
    writer: W,
    methods: RpcModule,
    rpc_middleware: &RpcServiceBuilder<L>,
    config: &ServerConfig,
    stop: StopHandle,
    mut extensions: Extensions,
) where
    R: Stream<Item = Bytes> + Unpin,
    W: Sink<String> + Unpin,
    L: Layer<RpcService>,
    L::Service: RpcServiceT + 'static,
{
    let (tx, mut rx) = mpsc::channel::<String>(config.message_buffer_capacity.max(1) as usize);
    let conn_id = *extensions.get_or_insert_with(ConnectionId::next);
    let extensions = Arc::new(extensions);
    let conn = Arc::new(Connection::new(
        conn_id,
        tx.clone(),
        config.max_subscriptions_per_connection,
        config.id_provider.clone(),
    ));
    let max_request_size = config.max_request_body_size as usize;
    let max_response_size = config.max_response_body_size as usize;
    let batch_config = config.batch_config;
    let service =
        Arc::new(rpc_middleware.service(RpcService::new(methods, max_response_size, Some(conn))));
    let mut calls = JoinSet::new();

    let read = async {
        while let Some(msg) = reader.next().await {
            while calls.try_join_next().is_some() {}
            // Reserve the response slot first to apply backpressure on the reader.
            let Ok(permit) = tx.clone().reserve_owned().await else { break };
            if msg.len() > max_request_size {
                let err = reject_too_big_request(max_request_size);
                permit.send(MethodResponse::error(Id::Null, err).into_json());
                continue
            }
            let service = service.clone();
            let extensions = extensions.clone();
            calls.spawn(async move {
                if let Some((json, on_sent)) =
                    handle_message(&*service, msg, max_response_size, batch_config, &extensions)
                        .await
                {
                    permit.send(json);
                    for tx in on_sent {
                        let _ = tx.send(());
                    }
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
///
/// `extensions` are attached to every call and notification. The returned senders must be
/// notified once the response was queued on the connection.
pub(crate) async fn handle_message<S: RpcServiceT>(
    service: &S,
    msg: Bytes,
    max_response_size: usize,
    batch_config: BatchRequestConfig,
    extensions: &Extensions,
) -> Option<(String, Vec<oneshot::Sender<()>>)> {
    let mut on_sent = Vec::new();
    let entries = match split_batch(&msg) {
        None => {
            let response = match parse_message(&msg, extensions) {
                Message::Call(req) => service.call(req).await,
                Message::Notification(n) => {
                    service.notification(n).await;
                    return None
                }
                Message::Invalid(id, code) => MethodResponse::error(id, code),
            };
            return Some((response.into_json_with(&mut on_sent), on_sent))
        }
        Some(Ok(entries)) => entries,
        Some(Err(code)) => {
            return Some((MethodResponse::error(Id::Null, code).into_json(), on_sent))
        }
    };
    let err = match batch_config {
        BatchRequestConfig::Disabled => {
            Some(ErrorObject::borrowed(BATCHES_NOT_SUPPORTED_CODE, BATCHES_NOT_SUPPORTED_MSG))
        }
        BatchRequestConfig::Limit(limit) if entries.len() > limit as usize => {
            Some(reject_too_big_batch_request(limit as usize))
        }
        _ => None,
    };
    if let Some(err) = err {
        return Some((MethodResponse::error(Id::Null, err).into_json(), on_sent))
    }

    // Invalid entries are answered in place, calls are filled in after the batch completes.
    let mut responses = Vec::with_capacity(entries.len());
    let mut reqs = Vec::with_capacity(entries.len());
    let mut notifications = Vec::new();
    for entry in &entries {
        match parse_message(entry, extensions) {
            Message::Call(req) => {
                reqs.push(req);
                responses.push(None);
            }
            Message::Notification(n) => notifications.push(n),
            Message::Invalid(id, code) => responses.push(Some(MethodResponse::error(id, code))),
        }
    }
    drop(entries);
    let notify = join_all(notifications.into_iter().map(|n| service.notification(n)));
    let calls = async {
        if reqs.is_empty() {
            Vec::new()
        } else {
            service.batch(reqs).await
        }
    };
    let mut results = join(calls, notify).await.0.into_iter();
    let responses =
        responses.into_iter().filter_map(|response| response.or_else(|| results.next()));
    batch_json(responses, max_response_size, &mut on_sent).map(|json| (json, on_sent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{stop_channel, Notification, TrySendError};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    struct CountNotifications<S>(S, Arc<AtomicUsize>);

    impl<S: RpcServiceT> RpcServiceT for CountNotifications<S> {
        fn call(&self, req: crate::Request) -> impl Future<Output = MethodResponse> + Send {
            self.0.call(req)
        }

        async fn notification(&self, n: Notification) {
            assert_eq!(n.extensions().get::<ConnectionId>(), Some(&ConnectionId(7)));
            self.1.fetch_add(1, Ordering::Relaxed);
            self.0.notification(n).await
        }
    }

    async fn handle(service: &impl RpcServiceT, msg: &str, config: BatchRequestConfig) -> String {
        let mut extensions = Extensions::new();
        extensions.insert(ConnectionId(7));
        let msg = Bytes::copy_from_slice(msg.as_bytes());
        handle_message(service, msg, usize::MAX, config, &extensions).await.map_or_default(|r| r.0)
    }

    #[tokio::test]
    async fn batch_config_and_notifications() {
        let mut methods = RpcModule::new();
        methods.register_method("a", |_, _| 1).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let service = CountNotifications(RpcService::new(methods, usize::MAX, None), count.clone());

        let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"a"},{"jsonrpc":"2.0","method":"a"}]"#;
        assert_eq!(
            handle(&service, batch, BatchRequestConfig::Unlimited).await,
            r#"[{"jsonrpc":"2.0","id":1,"result":1}]"#
        );
        assert_eq!(
            handle(&service, batch, BatchRequestConfig::Limit(1)).await,
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32010,"message":"The batch request was too large","data":"Exceeded max limit of 1"}}"#
        );
        assert_eq!(
            handle(&service, batch, BatchRequestConfig::Disabled).await,
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32005,"message":"Batched requests are not supported by this server"}}"#
        );
        assert_eq!(
            handle(&service, r#"{"jsonrpc":"2.0","method":"a"}"#, BatchRequestConfig::Disabled)
                .await,
            ""
        );
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn try_send_until_full() {
        let (result_tx, result_rx) = oneshot::channel();
        let result_tx = Mutex::new(Some(result_tx));
        let mut methods = RpcModule::new();
        methods
            .register_subscription("sub", "notif", "unsub", move |_, pending, _| {
                let result_tx = result_tx.lock().unwrap().take().unwrap();
                async move {
                    let sink = pending.accept().await?;
                    while sink.try_send(&1).is_ok() {}
                    let res = (sink.try_send(&1), sink.capacity(), sink.max_capacity());
                    let _ = result_tx.send(res);
                    sink.closed().await;
                    Ok(())
                }
            })
            .unwrap();

        let (req_tx, req_rx) = futures::channel::mpsc::unbounded();
        // The writer is never read, so the connection buffer fills up.
        let (writer, _responses) = futures::channel::mpsc::channel::<String>(0);
        let config = ServerConfig::default().set_message_buffer_capacity(4);
        let (stop, _handle) = stop_channel();
        tokio::spawn(async move {
            let middleware = RpcServiceBuilder::new();
            serve_connection(req_rx, writer, methods, &middleware, &config, stop, Extensions::new())
                .await
        });
        req_tx
            .unbounded_send(Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"sub"}"#))
            .unwrap();
        assert_eq!(result_rx.await.unwrap(), (Err(TrySendError::Full), 0, 4));
    }
}
