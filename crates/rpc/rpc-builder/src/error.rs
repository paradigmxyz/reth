use std::net::SocketAddr;

use jsonrpsee::core::Error as JsonRpseeError;
use std::io::{Error as IoError, ErrorKind};

/// Rpc server kind.
#[derive(Debug)]
pub enum ServerKind {
    /// Http.
    Http(SocketAddr),
    /// Websocket.
    WS(SocketAddr),
    /// Auth.
    Auth(SocketAddr),
    /// Unknown.
    Unknown,
}

impl std::fmt::Display for ServerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerKind::Http(addr) => write!(f, "{addr} (HTTP-RPC server)"),
            ServerKind::WS(addr) => write!(f, "{addr} (WS-RPC server)"),
            ServerKind::Auth(addr) => write!(f, "{addr} (AUTH server)"),
            ServerKind::Unknown => write!(f, "(UNKNOWN server)"),
        }
    }
}

/// Rpc Errors.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// Wrapper for `jsonrpsee::core::Error`.
    #[error(transparent)]
    RpcError(JsonRpseeError),
    /// Address already in use.
    #[error("Address {kind} is already in use (os error 98)")]
    AddressAlreadyInUse {
        /// Server kind.
        kind: ServerKind,
        /// Io error.
        error: IoError,
    },
    /// Custom error.
    #[error("{0}")]
    Custom(String),
}

impl RpcError {
    /// Converts a `jsonrpsee::core::Error` to a more descriptive `RpcError`.
    pub fn from_jsonrpsee_error(error: JsonRpseeError, handle: Option<ServerKind>) -> RpcError {
        match error {
            JsonRpseeError::Transport(err) => match err.downcast::<IoError>() {
                Ok(io_error) => {
                    if io_error.kind() == ErrorKind::AddrInUse {
                        return RpcError::AddressAlreadyInUse {
                            kind: handle.unwrap_or(ServerKind::Unknown),
                            error: io_error,
                        }
                    }
                    RpcError::RpcError(JsonRpseeError::Transport(io_error.into()))
                }
                Err(error) => RpcError::RpcError(JsonRpseeError::Transport(error)),
            },
            _ => RpcError::RpcError(error),
        }
    }
}
