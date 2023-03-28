use std::net::SocketAddr;

use jsonrpsee::core::Error as JsonRpseeError;
use std::io::{Error as IoError, ErrorKind};

/// Handle type.
#[derive(Debug)]
pub enum RpcHandle {
    /// Http handle.
    Http(SocketAddr),
    /// Websocket handle.
    WS(SocketAddr),
    /// Auth handle.
    Auth(SocketAddr),
}

impl std::fmt::Display for RpcHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcHandle::Http(addr) => write!(f, "{addr} (HTTP-RPC server)"),
            RpcHandle::WS(addr) => write!(f, "{addr} (WS-RPC server)"),
            RpcHandle::Auth(addr) => write!(f, "{addr} (AUTH server)"),
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
    AddressAlreadyInUse(RpcHandle),
    /// Custom error.
    #[error("Custom error: {0}")]
    Custom(String),
}

impl RpcError {
    /// Converts a `jsonrpsee::core::Error` to a more descriptive `RpcError`.
    pub fn from_jsonrpsee_error(error: JsonRpseeError, handle: Option<RpcHandle>) -> RpcError {
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
