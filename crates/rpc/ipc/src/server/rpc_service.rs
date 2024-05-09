//! JSON-RPC service middleware.
use futures_util::future::BoxFuture;
use jsonrpsee::{
    server::{
        middleware::rpc::{ResponseFuture, RpcServiceT},
        IdProvider,
    },
    types::{error::reject_too_many_subscriptions, ErrorCode, ErrorObject, Request},
    BoundedSubscriptions, ConnectionDetails, MethodCallback, MethodResponse, MethodSink, Methods,
    SubscriptionState,
};
use std::sync::Arc;

/// JSON-RPC service middleware.
#[derive(Clone, Debug)]
pub struct RpcService {
    conn_id: usize,
    methods: Methods,
    max_response_body_size: usize,
    cfg: RpcServiceCfg,
}

/// Configuration of the RpcService.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum RpcServiceCfg {
    /// The server supports only calls.
    OnlyCalls,
    /// The server supports both method calls and subscriptions.
    CallsAndSubscriptions {
        bounded_subscriptions: BoundedSubscriptions,
        sink: MethodSink,
        id_provider: Arc<dyn IdProvider>,
    },
}

impl RpcService {
    /// Create a new service.
    pub(crate) fn new(
        methods: Methods,
        max_response_body_size: usize,
        conn_id: usize,
        cfg: RpcServiceCfg,
    ) -> Self {
        Self { methods, max_response_body_size, conn_id, cfg }
    }
}

impl<'a> RpcServiceT<'a> for RpcService {
    // The rpc module is already boxing the futures and
    // it's used to under the hood by the RpcService.
    type Future = ResponseFuture<BoxFuture<'a, MethodResponse>>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        let conn_id = self.conn_id;
        let max_response_body_size = self.max_response_body_size;

        let params = req.params();
        let name = req.method_name();
        let id = req.id().clone();

        match self.methods.method_with_name(name) {
            None => {
                let rp = MethodResponse::error(id, ErrorObject::from(ErrorCode::MethodNotFound));
                ResponseFuture::ready(rp)
            }
            Some((_name, method)) => match method {
                MethodCallback::Async(callback) => {
                    let params = params.into_owned();
                    let id = id.into_owned();

                    let fut = (callback)(id, params, conn_id, max_response_body_size);
                    ResponseFuture::future(fut)
                }
                MethodCallback::AsyncWithDetails(callback) => {
                    let params = params.into_owned();
                    let id = id.into_owned();

                    // Note: Add the `Request::extensions` to the connection details when available
                    // here.
                    let fut = (callback)(
                        id,
                        params,
                        ConnectionDetails::_new(conn_id),
                        max_response_body_size,
                    );
                    ResponseFuture::future(fut)
                }
                MethodCallback::Sync(callback) => {
                    let rp = (callback)(id, params, max_response_body_size);
                    ResponseFuture::ready(rp)
                }
                MethodCallback::Subscription(callback) => {
                    let RpcServiceCfg::CallsAndSubscriptions {
                        bounded_subscriptions,
                        sink,
                        id_provider,
                    } = self.cfg.clone()
                    else {
                        tracing::warn!("Subscriptions not supported");
                        let rp =
                            MethodResponse::error(id, ErrorObject::from(ErrorCode::InternalError));
                        return ResponseFuture::ready(rp);
                    };

                    if let Some(p) = bounded_subscriptions.acquire() {
                        let conn_state = SubscriptionState {
                            conn_id,
                            id_provider: &*id_provider.clone(),
                            subscription_permit: p,
                        };

                        let fut = callback(id.clone(), params, sink, conn_state);
                        ResponseFuture::future(fut)
                    } else {
                        let max = bounded_subscriptions.max();
                        let rp = MethodResponse::error(id, reject_too_many_subscriptions(max));
                        ResponseFuture::ready(rp)
                    }
                }
                MethodCallback::Unsubscription(callback) => {
                    // Don't adhere to any resource or subscription limits; always let unsubscribing
                    // happen!

                    let RpcServiceCfg::CallsAndSubscriptions { .. } = self.cfg else {
                        tracing::warn!("Subscriptions not supported");
                        let rp =
                            MethodResponse::error(id, ErrorObject::from(ErrorCode::InternalError));
                        return ResponseFuture::ready(rp);
                    };

                    let rp = callback(id, params, conn_id, max_response_body_size);
                    ResponseFuture::ready(rp)
                }
            },
        }
    }
}
