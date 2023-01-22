//! Additional helpers for converting errors.

use jsonrpsee::core::{Error as RpcError, RpcResult};
use reth_network_api::NetworkError;

/// Helper trait to easily convert various `Result` types into [`RpcResult`]
pub(crate) trait ToRpcResult<Ok, Err> {
    /// Converts this type into an [`RpcResult`]
    fn map_rpc_err<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(Err) -> (i32, M, Option<&'a [u8]>),
        M: Into<String>;

    /// Converts this type into an [`RpcResult`] with the
    /// [`jsonrpsee::types::error::INTERNAL_ERROR_CODE` and the given message.
    fn map_internal_err<F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(Err) -> M,
        M: Into<String>;

    /// Converts this type into an [`RpcResult`] with the
    /// [`jsonrpsee::types::error::INTERNAL_ERROR_CODE`] and given message and data.
    fn map_internal_err_with_data<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(Err) -> (M, &'a [u8]),
        M: Into<String>;

    /// Adds a message to the error variant and returns an internal Error.
    ///
    /// This is shorthand for `Self::map_internal_err(|err| format!("{msg}: {err}"))`.
    fn with_message(self, msg: &str) -> RpcResult<Ok>;
}

impl<Ok> ToRpcResult<Ok, reth_interfaces::Error> for reth_interfaces::Result<Ok> {
    #[inline]
    fn map_rpc_err<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(reth_interfaces::Error) -> (i32, M, Option<&'a [u8]>),
        M: Into<String>,
    {
        match self {
            Ok(t) => Ok(t),
            Err(err) => {
                let (code, msg, data) = op(err);
                Err(rpc_err(code, msg, data))
            }
        }
    }

    #[inline]
    fn map_internal_err<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(reth_interfaces::Error) -> M,
        M: Into<String>,
    {
        match self {
            Ok(t) => Ok(t),
            Err(err) => Err(internal_rpc_err(op(err))),
        }
    }

    #[inline]
    fn map_internal_err_with_data<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(reth_interfaces::Error) -> (M, &'a [u8]),
        M: Into<String>,
    {
        match self {
            Ok(t) => Ok(t),
            Err(err) => {
                let (msg, data) = op(err);
                Err(internal_rpc_err_with_data(msg, data))
            }
        }
    }

    #[inline]
    fn with_message(self, msg: &str) -> RpcResult<Ok> {
        match self {
            Ok(t) => Ok(t),
            Err(err) => {
                let msg = format!("{msg}: {err:?}");
                Err(internal_rpc_err(msg))
            }
        }
    }
}

impl<Ok> ToRpcResult<Ok, NetworkError> for Result<Ok, NetworkError> {
    #[inline]
    fn map_rpc_err<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(NetworkError) -> (i32, M, Option<&'a [u8]>),
        M: Into<String>,
    {
        match self {
            Ok(t) => Ok(t),
            Err(err) => {
                let (code, msg, data) = op(err);
                Err(rpc_err(code, msg, data))
            }
        }
    }

    #[inline]
    fn map_internal_err<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(NetworkError) -> M,
        M: Into<String>,
    {
        match self {
            Ok(t) => Ok(t),
            Err(err) => Err(internal_rpc_err(op(err))),
        }
    }

    #[inline]
    fn map_internal_err_with_data<'a, F, M>(self, op: F) -> RpcResult<Ok>
    where
        F: FnOnce(NetworkError) -> (M, &'a [u8]),
        M: Into<String>,
    {
        match self {
            Ok(t) => Ok(t),
            Err(err) => {
                let (msg, data) = op(err);
                Err(internal_rpc_err_with_data(msg, data))
            }
        }
    }

    #[inline]
    fn with_message(self, msg: &str) -> RpcResult<Ok> {
        match self {
            Ok(t) => Ok(t),
            Err(err) => {
                let msg = format!("{msg}: {err:?}");
                Err(internal_rpc_err(msg))
            }
        }
    }
}

/// Constructs an internal JSON-RPC error.
pub(crate) fn internal_rpc_err(msg: impl Into<String>) -> jsonrpsee::core::Error {
    rpc_err(jsonrpsee::types::error::INTERNAL_ERROR_CODE, msg, None)
}

/// Constructs an internal JSON-RPC error with data
pub(crate) fn internal_rpc_err_with_data(
    msg: impl Into<String>,
    data: &[u8],
) -> jsonrpsee::core::Error {
    rpc_err(jsonrpsee::types::error::INTERNAL_ERROR_CODE, msg, Some(data))
}

/// Constructs a JSON-RPC error, consisting of `code`, `message` and optional `data`.
pub(crate) fn rpc_err(code: i32, msg: impl Into<String>, data: Option<&[u8]>) -> RpcError {
    RpcError::Call(jsonrpsee::types::error::CallError::Custom(
        jsonrpsee::types::error::ErrorObject::owned(
            code,
            msg.into(),
            data.map(|data| {
                jsonrpsee::core::to_json_raw_value(&format!("0x{}", hex::encode(data)))
                    .expect("serializing String does fail")
            }),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rpc_result<Ok, Err, T: ToRpcResult<Ok, Err>>() {}

    fn to_reth_err<Ok>(o: Ok) -> reth_interfaces::Result<Ok> {
        Ok(o)
    }

    #[test]
    fn can_convert_rpc() {
        assert_rpc_result::<(), reth_interfaces::Error, reth_interfaces::Result<()>>();
        let res = to_reth_err(100);

        let rpc_res = res.map_internal_err(|_| "This is a message");
        let val = rpc_res.unwrap();
        assert_eq!(val, 100);
    }
}
