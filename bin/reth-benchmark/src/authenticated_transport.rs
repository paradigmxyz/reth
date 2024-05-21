//! This contains an authenticated rpc transport that can be used to send engine API newPayload
//! requests.

use std::sync::Arc;

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_pubsub::{PubSubConnect, PubSubFrontend};
use alloy_rpc_types_engine::{Claims, JwtSecret};
use alloy_transport::{
    Authorization, BoxTransport, TransportError, TransportErrorKind, TransportFut,
};
use alloy_transport_http::{reqwest::Url, Http, ReqwestTransport};
use alloy_transport_ipc::IpcConnect;
use alloy_transport_ws::WsConnect;
use std::task::{Context, Poll};
use tokio::sync::RwLock;

/// An enum representing the different transports that can be used to connect to a runtime.
/// Only meant to be used internally by [AuthenticatedTransport].
#[derive(Clone, Debug)]
pub enum InnerTransport {
    /// HTTP transport
    Http(ReqwestTransport),
    /// WebSocket transport
    Ws(PubSubFrontend),
    /// IPC transport
    Ipc(PubSubFrontend),
}

#[derive(Debug)]
pub struct AuthenticatedTransport {
    /// The inner actual transport used.
    inner: Arc<RwLock<InnerTransport>>,
    /// The URL to connect to.
    url: Url,
    /// The jwt
    jwt: String,
}

/// An error that can occur when creating an authenticated transport.
#[derive(Debug, thiserror::Error)]
pub enum AuthenticatedTransportError {
    /// The URL is invalid.
    #[error("The URL is invalid")]
    InvalidUrl,
    /// Failed to lock transport
    #[error("Failed to lock transport")]
    LockFailed,
    /// The JWT is invalid.
    #[error("The JWT is invalid")]
    InvalidJwt,
    /// The transport failed to connect.
    #[error("The transport failed to connect")]
    TransportFailed,
}

impl AuthenticatedTransport {
    /// Connects to an HTTP [alloy_transport_http::Http] transport.
    async fn connect_http(&self) -> Result<InnerTransport, AuthenticatedTransportError> {
        todo!()
    }

    /// Connects to a WebSocket [alloy_transport_ws::WsConnect] transport.
    async fn connect_ws(&self) -> Result<InnerTransport, AuthenticatedTransportError> {
        todo!()
    }

    /// Connects to an IPC [alloy_transport_ipc::IpcConnect] transport.
    async fn connect_ipc(&self) -> Result<InnerTransport, AuthenticatedTransportError> {
        todo!()
    }

    /// Sends a request using the underlying transport.
    ///
    /// For sending the actual request, this action is delegated down to the underlying transport
    /// through Tower's [tower::Service::call]. See tower's [tower::Service] trait for more
    /// information.
    fn request(&self, req: RequestPacket) -> TransportFut<'static> {
        todo!()
    }
}

// impl tower::Service<RequestPacket> for AuthenticatedTransport {
//     type Response = ResponsePacket;
//     type Error = TransportError;
//     type Future = TransportFut<'static>;

//     fn poll_ready(
//         &mut self,
//         _cx: &mut Context<'_>,
//     ) -> Poll<Result<(), Self::Error>> {
//         Poll::Ready(Ok(()))
//     }

//     fn call(&mut self, req: RequestPacket) -> Self::Future {
//         self.request(req)
//     }
// }

// impl tower::Service<RequestPacket> for AuthenticatedTransport {
//     type Response = ResponsePacket;
//     type Error = TransportError;
//     type Future = TransportFut<'static>;

//     fn poll_ready(
//         &mut self,
//         _cx: &mut Context<'_>,
//     ) -> Poll<Result<(), Self::Error>> {
//         Poll::Ready(Ok(()))
//     }

//     fn call(&mut self, req: RequestPacket) -> Self::Future {
//         self.request(req)
//     }
// }
