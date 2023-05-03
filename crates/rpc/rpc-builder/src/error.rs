use std::net::SocketAddr;

use crate::RethRpcModule;
use jsonrpsee::core::Error as JsonRpseeError;
use std::{io, io::ErrorKind};

/// Rpc server kind.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ServerKind {
    /// Http.
    Http(SocketAddr),
    /// Websocket.
    WS(SocketAddr),
    /// WS and http on the same port
    WsHttp(SocketAddr),
    /// Auth.
    Auth(SocketAddr),
}

impl std::fmt::Display for ServerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerKind::Http(addr) => write!(f, "{addr} (HTTP-RPC server)"),
            ServerKind::WS(addr) => write!(f, "{addr} (WS-RPC server)"),
            ServerKind::WsHttp(addr) => write!(f, "{addr} (WS-HTTP-RPC server)"),
            ServerKind::Auth(addr) => write!(f, "{addr} (AUTH server)"),
        }
    }
}

/// Rpc Errors.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// Wrapper for `jsonrpsee::core::Error`.
    #[error(transparent)]
    RpcError(#[from] JsonRpseeError),
    /// Address already in use.
    #[error("Address {kind} is already in use (os error 98)")]
    AddressAlreadyInUse {
        /// Server kind.
        kind: ServerKind,
        /// IO error.
        error: io::Error,
    },
    /// Http and WS server configured on the same port but with conflicting settings.
    #[error(transparent)]
    WsHttpSamePortError(#[from] WsHttpSamePortError),
    /// Custom error.
    #[error("{0}")]
    Custom(String),
}

impl RpcError {
    /// Converts a `jsonrpsee::core::Error` to a more descriptive `RpcError`.
    pub fn from_jsonrpsee_error(err: JsonRpseeError, kind: ServerKind) -> RpcError {
        match err {
            JsonRpseeError::Transport(err) => {
                if let Some(io_error) = err.downcast_ref::<io::Error>() {
                    if io_error.kind() == ErrorKind::AddrInUse {
                        return RpcError::AddressAlreadyInUse {
                            kind,
                            error: io::Error::from(io_error.kind()),
                        }
                    }
                }
                RpcError::RpcError(err.into())
            }
            _ => err.into(),
        }
    }
}

/// Errors when trying to launch ws and http server on the same port.
#[derive(Debug, thiserror::Error)]
pub enum WsHttpSamePortError {
    /// Ws and http server configured on same port but with different cors domains.
    #[error("CORS domains for http and ws are different, but they are on the same port: http: {http_cors_domains:?}, ws: {ws_cors_domains:?}")]
    ConflictingCorsDomains {
        /// Http cors domains.
        http_cors_domains: Option<String>,
        /// Ws cors domains.
        ws_cors_domains: Option<String>,
    },
    /// Ws and http server configured on same port but with different modules.
    #[error("Different api modules for http and ws on the same port is currently not supported: http: {http_modules:?}, ws: {ws_modules:?}")]
    ConflictingModules {
        /// Http modules.
        http_modules: Vec<RethRpcModule>,
        /// Ws modules.
        ws_modules: Vec<RethRpcModule>,
    },
}
