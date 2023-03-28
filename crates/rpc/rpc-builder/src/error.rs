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
}

impl std::fmt::Display for ServerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerKind::Http(addr) => write!(f, "{addr} (HTTP-RPC server)"),
            ServerKind::WS(addr) => write!(f, "{addr} (WS-RPC server)"),
            ServerKind::Auth(addr) => write!(f, "{addr} (AUTH server)"),
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
    #[error("Address {0} is already in use (os error 98)")]
    AddressAlreadyInUse(ServerKind),
    /// Custom error.
    #[error("{0}")]
    Custom(String),
}

impl RpcError {
    /// Converts a `jsonrpsee::core::Error` to a more descriptive `RpcError`.
    pub fn from_jsonrpsee_error(error: JsonRpseeError, handle: Option<ServerKind>) -> RpcError {
        match error {
            JsonRpseeError::Transport(error) => {
                if let Some(io_error) = error.downcast_ref::<IoError>() {
                    if io_error.kind() == ErrorKind::AddrInUse {
                        return RpcError::AddressAlreadyInUse(handle.unwrap())
                    }
                }
                RpcError::RpcError(JsonRpseeError::Transport(error))
            }
            _ => RpcError::RpcError(error),
        }
    }
}
