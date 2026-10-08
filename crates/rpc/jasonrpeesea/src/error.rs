use serde::{Deserialize, Serialize};
use serde_json::value::{to_raw_value, RawValue};
use std::{borrow::Cow, fmt};

/// Parse error code.
pub const PARSE_ERROR_CODE: i32 = -32700;
/// Invalid request error code.
pub const INVALID_REQUEST_CODE: i32 = -32600;
/// Method not found error code.
pub const METHOD_NOT_FOUND_CODE: i32 = -32601;
/// Invalid params error code.
pub const INVALID_PARAMS_CODE: i32 = -32602;
/// Internal error code.
pub const INTERNAL_ERROR_CODE: i32 = -32603;
/// Call execution failed error code.
pub const CALL_EXECUTION_FAILED_CODE: i32 = -32000;
/// Too many subscriptions error code.
pub const TOO_MANY_SUBSCRIPTIONS_CODE: i32 = -32006;
/// Oversized request error code.
pub const OVERSIZED_REQUEST_CODE: i32 = -32007;
/// Oversized response error code.
pub const OVERSIZED_RESPONSE_CODE: i32 = -32008;
/// Server is busy error code.
pub const SERVER_IS_BUSY_CODE: i32 = -32009;
/// Batch response too big error code.
pub const TOO_BIG_BATCH_RESPONSE_CODE: i32 = -32011;

/// Parse error message.
pub const PARSE_ERROR_MSG: &str = "Parse error";
/// Invalid request error message.
pub const INVALID_REQUEST_MSG: &str = "Invalid request";
/// Method not found error message.
pub const METHOD_NOT_FOUND_MSG: &str = "Method not found";
/// Invalid params error message.
pub const INVALID_PARAMS_MSG: &str = "Invalid params";
/// Internal error message.
pub const INTERNAL_ERROR_MSG: &str = "Internal error";
/// Generic server error message.
pub const SERVER_ERROR_MSG: &str = "Server error";
/// Too many subscriptions error message.
pub const TOO_MANY_SUBSCRIPTIONS_MSG: &str = "Too many subscriptions on the connection";
/// Oversized request error message.
pub const OVERSIZED_REQUEST_MSG: &str = "Request is too big";
/// Oversized response error message.
pub const OVERSIZED_RESPONSE_MSG: &str = "Response is too big";
/// Server is busy error message.
pub const SERVER_IS_BUSY_MSG: &str = "Server is busy, try again later";
/// Batch response too big error message.
pub const TOO_BIG_BATCH_RESPONSE_MSG: &str = "The batch response was too large";

/// [JSON-RPC error object](https://www.jsonrpc.org/specification#error_object).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorObject {
    code: i32,
    message: Cow<'static, str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Box<RawValue>>,
}

impl ErrorObject {
    /// Creates a new error object, serializing `data` if present.
    pub fn owned<S: Serialize>(code: i32, message: impl Into<String>, data: Option<S>) -> Self {
        let data = data.and_then(|data| to_raw_value(&data).ok());
        Self { code, message: Cow::Owned(message.into()), data }
    }

    /// Creates a new error object with a static message and no data.
    pub const fn borrowed(code: i32, message: &'static str) -> Self {
        Self { code, message: Cow::Borrowed(message), data: None }
    }

    /// Creates a new error object with already serialized data.
    pub fn with_raw_data(
        code: i32,
        message: impl Into<Cow<'static, str>>,
        data: Option<Box<RawValue>>,
    ) -> Self {
        Self { code, message: message.into(), data }
    }

    /// Returns the error code.
    pub const fn code(&self) -> i32 {
        self.code
    }

    /// Returns the error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the error data, if any.
    pub fn data(&self) -> Option<&RawValue> {
        self.data.as_deref()
    }
}

impl PartialEq for ErrorObject {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code &&
            self.message == other.message &&
            self.data.as_ref().map(|d| d.get()) == other.data.as_ref().map(|d| d.get())
    }
}

impl fmt::Display for ErrorObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(data) = &self.data {
            write!(f, ", data: {}", data.get())?;
        }
        Ok(())
    }
}

impl core::error::Error for ErrorObject {}

impl From<ErrorCode> for ErrorObject {
    fn from(code: ErrorCode) -> Self {
        Self::borrowed(code.code(), code.message())
    }
}

impl From<core::convert::Infallible> for ErrorObject {
    fn from(value: core::convert::Infallible) -> Self {
        match value {}
    }
}

/// Standard JSON-RPC error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// Invalid JSON was received by the server.
    ParseError,
    /// The request was too big.
    OversizedRequest,
    /// The JSON sent is not a valid request object.
    InvalidRequest,
    /// The method does not exist or is not available.
    MethodNotFound,
    /// The server is busy.
    ServerIsBusy,
    /// Invalid method parameters.
    InvalidParams,
    /// Internal JSON-RPC error.
    InternalError,
    /// Implementation-defined server error.
    ServerError(i32),
}

impl ErrorCode {
    /// Returns the integer code.
    pub const fn code(&self) -> i32 {
        match self {
            Self::ParseError => PARSE_ERROR_CODE,
            Self::OversizedRequest => OVERSIZED_REQUEST_CODE,
            Self::InvalidRequest => INVALID_REQUEST_CODE,
            Self::MethodNotFound => METHOD_NOT_FOUND_CODE,
            Self::ServerIsBusy => SERVER_IS_BUSY_CODE,
            Self::InvalidParams => INVALID_PARAMS_CODE,
            Self::InternalError => INTERNAL_ERROR_CODE,
            Self::ServerError(code) => *code,
        }
    }

    /// Returns the default message.
    pub const fn message(&self) -> &'static str {
        match self {
            Self::ParseError => PARSE_ERROR_MSG,
            Self::OversizedRequest => OVERSIZED_REQUEST_MSG,
            Self::InvalidRequest => INVALID_REQUEST_MSG,
            Self::MethodNotFound => METHOD_NOT_FOUND_MSG,
            Self::ServerIsBusy => SERVER_IS_BUSY_MSG,
            Self::InvalidParams => INVALID_PARAMS_MSG,
            Self::InternalError => INTERNAL_ERROR_MSG,
            Self::ServerError(_) => SERVER_ERROR_MSG,
        }
    }
}

impl From<i32> for ErrorCode {
    fn from(code: i32) -> Self {
        match code {
            PARSE_ERROR_CODE => Self::ParseError,
            OVERSIZED_REQUEST_CODE => Self::OversizedRequest,
            INVALID_REQUEST_CODE => Self::InvalidRequest,
            METHOD_NOT_FOUND_CODE => Self::MethodNotFound,
            SERVER_IS_BUSY_CODE => Self::ServerIsBusy,
            INVALID_PARAMS_CODE => Self::InvalidParams,
            INTERNAL_ERROR_CODE => Self::InternalError,
            code => Self::ServerError(code),
        }
    }
}

/// Returns an invalid params error with the given reason as data.
pub fn invalid_params(reason: impl fmt::Display) -> ErrorObject {
    ErrorObject::owned(INVALID_PARAMS_CODE, INVALID_PARAMS_MSG, Some(reason.to_string()))
}

pub(crate) fn exceeded_limit(code: i32, message: &'static str, limit: usize) -> ErrorObject {
    ErrorObject::owned(code, message, Some(format!("Exceeded max limit of {limit}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let err = ErrorObject::owned(-1, "msg", Some(1));
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"code":-1,"message":"msg","data":1}"#);
        assert_eq!(serde_json::from_str::<ErrorObject>(&json).unwrap(), err);
        assert_eq!(err.to_string(), "-1: msg, data: 1");

        let err = ErrorObject::from(ErrorCode::MethodNotFound);
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"code":-32601,"message":"Method not found"}"#
        );
    }
}
