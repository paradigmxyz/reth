use std::net::SocketAddr;

use jsonrpsee::core::Error as JsonRpseeError;
use std::{io, io::ErrorKind};

/// Rpc server kind.
#[derive(Debug, PartialEq)]
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
    RpcError(#[from] JsonRpseeError),
    /// Address already in use.
    #[error("Address {kind} is already in use (os error 98)")]
    AddressAlreadyInUse {
        /// Server kind.
        kind: ServerKind,
        /// IO error.
        error: io::Error,
    },
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
