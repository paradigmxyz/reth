//! JSON-RPC clients.

use crate::{ErrorObject, SubscriptionId};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use std::{fmt::Write as _, future::Future};

mod async_client;
pub use async_client::{Client, ClientBuilder, Subscription};

#[cfg(feature = "http-client")]
mod http;
#[cfg(feature = "http-client")]
pub use http::{HttpClient, HttpClientBuilder};

#[cfg(feature = "ws-client")]
mod ws;
#[cfg(feature = "ws-client")]
pub use ws::{WsClient, WsClientBuilder};

/// Boxed error type of transports.
pub type BoxError = Box<dyn core::error::Error + Send + Sync>;

/// Error returned by clients.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server answered with an error.
    #[error("{0}")]
    Call(ErrorObject),
    /// The transport failed.
    #[error("transport error: {0}")]
    Transport(BoxError),
    /// The connection to the server was closed.
    #[error("connection closed")]
    Closed,
    /// The request timed out.
    #[error("request timed out")]
    RequestTimeout,
    /// A request or response could not be serialized or deserialized.
    #[error("parse error: {0}")]
    ParseError(#[from] serde_json::Error),
    /// The operation is not supported by the transport.
    #[error("{0} is not supported by this transport")]
    Unsupported(&'static str),
}

/// A JSON-RPC client.
pub trait ClientT: Send + Sync {
    /// Sends a method call and returns its result.
    fn request<R, P>(
        &self,
        method: &str,
        params: P,
    ) -> impl Future<Output = Result<R, Error>> + Send
    where
        R: DeserializeOwned,
        P: ToRpcParams + Send;

    /// Sends a batch of method calls and returns their results in order.
    fn batch_request<R>(
        &self,
        batch: BatchRequestBuilder,
    ) -> impl Future<Output = Result<Vec<Result<R, ErrorObject>>, Error>> + Send
    where
        R: DeserializeOwned;
}

/// A JSON-RPC client that supports subscriptions.
pub trait SubscriptionClientT: ClientT {
    /// Subscribes with `subscribe` and returns a stream of notifications.
    ///
    /// `unsubscribe` is called when the subscription is dropped.
    fn subscribe<N, P>(
        &self,
        subscribe: &str,
        params: P,
        unsubscribe: &str,
    ) -> impl Future<Output = Result<Subscription<N>, Error>> + Send
    where
        N: DeserializeOwned,
        P: ToRpcParams + Send;
}

/// Serializes method parameters.
pub trait ToRpcParams {
    /// Returns the JSON parameters, or `None` to omit them.
    fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error>;
}

impl ToRpcParams for Option<Box<RawValue>> {
    fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
        Ok(self)
    }
}

macro_rules! impl_to_rpc_params_tuple {
    ($($ty:ident),+) => {
        impl<$($ty: Serialize),+> ToRpcParams for ($($ty,)+) {
            fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
                serde_json::value::to_raw_value(&self).map(Some)
            }
        }
    };
}

impl_to_rpc_params_tuple!(A);
impl_to_rpc_params_tuple!(A, B);
impl_to_rpc_params_tuple!(A, B, C);
impl_to_rpc_params_tuple!(A, B, C, D);
impl_to_rpc_params_tuple!(A, B, C, D, E);
impl_to_rpc_params_tuple!(A, B, C, D, E, F);

impl<P: Serialize, const N: usize> ToRpcParams for [P; N] {
    fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
        serde_json::value::to_raw_value(self.as_slice()).map(Some)
    }
}

impl<P: Serialize> ToRpcParams for Vec<P> {
    fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
        serde_json::value::to_raw_value(&self).map(Some)
    }
}

/// Positional parameters, usually built with [`rpc_params!`](crate::rpc_params).
#[derive(Clone, Debug, Default)]
pub struct ArrayParams(String);

impl ArrayParams {
    /// Creates empty parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a parameter.
    pub fn insert<P: Serialize>(&mut self, value: P) -> Result<(), serde_json::Error> {
        self.0.push(if self.0.is_empty() { '[' } else { ',' });
        // SAFETY: `serde_json` only writes valid UTF-8.
        serde_json::to_writer(unsafe { self.0.as_mut_vec() }, &value)
    }
}

impl ToRpcParams for ArrayParams {
    fn to_rpc_params(mut self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
        if self.0.is_empty() {
            return Ok(None)
        }
        self.0.push(']');
        RawValue::from_string(self.0).map(Some)
    }
}

/// Builds [`ArrayParams`] from the given values, panicking if any fails to serialize.
#[macro_export]
macro_rules! rpc_params {
    ($($param:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut params = $crate::client::ArrayParams::new();
        $(
            if let Err(err) = params.insert($param) {
                panic!("parameter `{}` cannot be serialized: {err}", stringify!($param));
            }
        )*
        params
    }};
}

/// Builds a batch of method calls.
#[derive(Clone, Debug, Default)]
pub struct BatchRequestBuilder(Vec<(String, Option<Box<RawValue>>)>);

impl BatchRequestBuilder {
    /// Creates an empty batch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a method call.
    pub fn insert(
        &mut self,
        method: impl Into<String>,
        params: impl ToRpcParams,
    ) -> Result<(), serde_json::Error> {
        self.0.push((method.into(), params.to_rpc_params()?));
        Ok(())
    }

    /// Returns `true` if the batch is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Serializes a request with the given id.
fn write_request(buf: &mut String, id: u64, method: &str, params: Option<&RawValue>) {
    let _ = write!(buf, r#"{{"jsonrpc":"2.0","id":{id},"method":"#);
    // SAFETY: `serde_json` only writes valid UTF-8.
    let _ = serde_json::to_writer(unsafe { buf.as_mut_vec() }, method);
    if let Some(params) = params {
        buf.push_str(r#","params":"#);
        buf.push_str(params.get());
    }
    buf.push('}');
}

/// Serializes a batch of requests with consecutive ids starting at `first_id`.
fn write_batch(batch: &BatchRequestBuilder, first_id: u64) -> String {
    let mut buf = String::from("[");
    for (i, (method, params)) in batch.0.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        write_request(&mut buf, first_id + i as u64, method, params.as_deref());
    }
    buf.push(']');
    buf
}

/// A response or notification received by a client.
#[derive(Deserialize)]
struct RawResponse<'a> {
    #[serde(default)]
    id: Option<u64>,
    #[serde(borrow, default, deserialize_with = "some_raw")]
    result: Option<&'a RawValue>,
    #[serde(default)]
    error: Option<ErrorObject>,
    #[serde(borrow, default)]
    method: Option<&'a str>,
    #[serde(borrow, default)]
    params: Option<NotificationParams<'a>>,
}

#[derive(Deserialize)]
struct NotificationParams<'a> {
    subscription: SubscriptionId,
    #[serde(borrow)]
    result: &'a RawValue,
}

impl RawResponse<'_> {
    /// Returns the result of a method call.
    fn into_result(self) -> Result<Box<RawValue>, ErrorObject> {
        match (self.error, self.result) {
            (Some(err), _) => Err(err),
            (None, Some(result)) => Ok(result.to_owned()),
            (None, None) => Err(crate::ErrorCode::InvalidRequest.into()),
        }
    }
}

/// Keeps a `null` result as `Some`, so that only a missing field yields `None`.
fn some_raw<'de: 'a, 'a, D: Deserializer<'de>>(d: D) -> Result<Option<&'a RawValue>, D::Error> {
    <&RawValue>::deserialize(d).map(Some)
}

fn decode<R: DeserializeOwned>(result: Result<Box<RawValue>, ErrorObject>) -> Result<R, Error> {
    serde_json::from_str(result.map_err(Error::Call)?.get()).map_err(Error::ParseError)
}

fn decode_batch<R: DeserializeOwned>(
    results: Vec<Result<Box<RawValue>, ErrorObject>>,
) -> Result<Vec<Result<R, ErrorObject>>, Error> {
    results
        .into_iter()
        .map(|result| match result {
            Ok(value) => serde_json::from_str(value.get()).map(Ok).map_err(Error::ParseError),
            Err(err) => Ok(Err(err)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params() {
        assert!(crate::rpc_params![].to_rpc_params().unwrap().is_none());
        let params = crate::rpc_params![1, "a", None::<u8>].to_rpc_params().unwrap().unwrap();
        assert_eq!(params.get(), r#"[1,"a",null]"#);

        let mut buf = String::new();
        write_request(&mut buf, 7, "m", Some(&params));
        assert_eq!(buf, r#"{"jsonrpc":"2.0","id":7,"method":"m","params":[1,"a",null]}"#);

        let mut batch = BatchRequestBuilder::new();
        batch.insert("a", crate::rpc_params![]).unwrap();
        batch.insert("b", crate::rpc_params![1]).unwrap();
        assert_eq!(
            write_batch(&batch, 3),
            r#"[{"jsonrpc":"2.0","id":3,"method":"a"},{"jsonrpc":"2.0","id":4,"method":"b","params":[1]}]"#
        );
    }

    #[test]
    fn null_result() {
        let response =
            serde_json::from_str::<RawResponse<'_>>(r#"{"jsonrpc":"2.0","id":1,"result":null}"#)
                .unwrap();
        assert_eq!(response.into_result().unwrap().get(), "null");

        let response =
            serde_json::from_str::<RawResponse<'_>>(r#"{"jsonrpc":"2.0","id":1}"#).unwrap();
        assert!(response.into_result().is_err());
    }
}
