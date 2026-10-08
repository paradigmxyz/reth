use crate::{
    error::exceeded_limit, ErrorCode, ErrorObject, Id, OVERSIZED_RESPONSE_CODE,
    OVERSIZED_RESPONSE_MSG, TOO_BIG_BATCH_RESPONSE_CODE, TOO_BIG_BATCH_RESPONSE_MSG,
};
use serde::Serialize;
use std::io;

const PREFIX: &[u8] = br#"{"jsonrpc":"2.0","id":"#;

/// A serialized JSON-RPC response to a method call.
#[derive(Clone, Debug)]
pub struct MethodResponse {
    json: String,
    error_code: Option<i32>,
    subscription: bool,
}

impl MethodResponse {
    /// Serializes a successful response.
    ///
    /// Returns an oversized response error if the response exceeds `max_size` bytes, or an
    /// internal error if `result` fails to serialize.
    pub fn response<T: Serialize + ?Sized>(id: Id, result: &T, max_size: usize) -> Self {
        let mut w = BoundedWriter::new(max_size);
        w.buf.extend_from_slice(PREFIX);
        id.write_json(&mut w.buf);
        w.buf.extend_from_slice(br#","result":"#);
        match serde_json::to_writer(&mut w, result)
            .and_then(|()| io::Write::write_all(&mut w, b"}").map_err(serde_json::Error::io))
        {
            // SAFETY: `serde_json` only writes valid UTF-8.
            Ok(()) => Self::new(unsafe { String::from_utf8_unchecked(w.buf) }, None),
            Err(err) if err.is_io() => Self::error(
                id,
                exceeded_limit(OVERSIZED_RESPONSE_CODE, OVERSIZED_RESPONSE_MSG, max_size),
            ),
            Err(err) => {
                tracing::error!(target: "rpc::jsonrpc", %err, "failed to serialize response");
                Self::error(id, ErrorCode::InternalError)
            }
        }
    }

    /// Serializes an error response.
    pub fn error(id: Id, err: impl Into<ErrorObject>) -> Self {
        let err = err.into();
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(PREFIX);
        id.write_json(&mut buf);
        buf.extend_from_slice(br#","error":"#);
        let _ = serde_json::to_writer(&mut buf, &err);
        buf.push(b'}');
        // SAFETY: `serde_json` only writes valid UTF-8.
        Self::new(unsafe { String::from_utf8_unchecked(buf) }, Some(err.code()))
    }

    /// Serializes `result` as a successful or error response.
    pub fn from_result<T, E>(id: Id, result: Result<T, E>, max_size: usize) -> Self
    where
        T: Serialize,
        E: Into<ErrorObject>,
    {
        match result {
            Ok(value) => Self::response(id, &value, max_size),
            Err(err) => Self::error(id, err),
        }
    }

    /// Marks a subscription that was accepted and already answered on the connection.
    pub(crate) const fn subscription_accepted() -> Self {
        Self { json: String::new(), error_code: None, subscription: true }
    }

    const fn new(json: String, error_code: Option<i32>) -> Self {
        Self { json, error_code, subscription: false }
    }

    /// Returns `true` if the call succeeded.
    pub const fn is_success(&self) -> bool {
        self.error_code.is_none()
    }

    /// Returns `true` if the call failed.
    pub const fn is_error(&self) -> bool {
        self.error_code.is_some()
    }

    /// Returns the error code if the call failed.
    pub const fn as_error_code(&self) -> Option<i32> {
        self.error_code
    }

    /// Returns `true` for an accepted subscription, whose response was already sent.
    pub const fn is_subscription(&self) -> bool {
        self.subscription
    }

    /// Returns the serialized response.
    pub fn as_json(&self) -> &str {
        &self.json
    }

    /// Consumes the response and returns the serialized JSON.
    pub fn into_json(self) -> String {
        self.json
    }
}

/// Joins batch responses into a JSON array, or returns an error if it exceeds `max_size` bytes.
///
/// Returns `None` if no response has to be sent.
pub(crate) fn batch_json(
    responses: impl IntoIterator<Item = MethodResponse>,
    max_size: usize,
) -> Option<String> {
    let mut json = String::with_capacity(128);
    json.push('[');
    for response in responses {
        if response.is_subscription() {
            continue
        }
        if json.len() + response.json.len() + 1 > max_size {
            let err =
                exceeded_limit(TOO_BIG_BATCH_RESPONSE_CODE, TOO_BIG_BATCH_RESPONSE_MSG, max_size);
            return Some(MethodResponse::error(Id::Null, err).into_json())
        }
        if json.len() > 1 {
            json.push(',');
        }
        json.push_str(&response.json);
    }
    if json.len() == 1 {
        return None
    }
    json.push(']');
    Some(json)
}

struct BoundedWriter {
    buf: Vec<u8>,
    max: usize,
}

impl BoundedWriter {
    fn new(max: usize) -> Self {
        Self { buf: Vec::with_capacity(128.min(max)), max }
    }
}

impl io::Write for BoundedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len() + data.len() > self.max {
            return Err(io::ErrorKind::OutOfMemory.into())
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response() {
        let res = MethodResponse::response(Id::Number(1), "a", 100);
        assert_eq!(res.as_json(), r#"{"jsonrpc":"2.0","id":1,"result":"a"}"#);
        assert!(res.is_success());

        let res = MethodResponse::response(Id::Str("x".into()), &"a".repeat(100), 100);
        assert_eq!(res.as_error_code(), Some(OVERSIZED_RESPONSE_CODE));
        assert_eq!(
            res.as_json(),
            r#"{"jsonrpc":"2.0","id":"x","error":{"code":-32008,"message":"Response is too big","data":"Exceeded max limit of 100"}}"#
        );

        let res = MethodResponse::error(Id::Null, ErrorCode::MethodNotFound);
        assert_eq!(
            res.as_json(),
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32601,"message":"Method not found"}}"#
        );
    }

    #[test]
    fn batch() {
        let a = MethodResponse::response(Id::Number(1), &1, 100);
        let b = MethodResponse::response(Id::Number(2), &2, 100);
        assert_eq!(
            batch_json([a.clone(), b], 100).unwrap(),
            r#"[{"jsonrpc":"2.0","id":1,"result":1},{"jsonrpc":"2.0","id":2,"result":2}]"#
        );
        assert!(batch_json([a], 10).unwrap().contains("-32011"));
        assert_eq!(batch_json([MethodResponse::subscription_accepted()], 10), None);
    }
}
