// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Types pertaining to JSON-RPC responses.

use std::borrow::Cow as StdCow;
use std::fmt;
use std::marker::PhantomData;

use crate::error::ErrorCode;
use crate::params::{Id, SubscriptionId, TwoPointZero};
use crate::request::Notification;
use crate::{ErrorObject, ErrorObjectOwned};
use http::Extensions;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// JSON-RPC response object as defined in the [spec](https://www.jsonrpc.org/specification#response_object).
pub struct Response<'a, T: Clone> {
	/// JSON-RPC version.
	pub jsonrpc: Option<TwoPointZero>,
	/// Payload which can be result or error.
	pub payload: ResponsePayload<'a, T>,
	/// Request ID
	pub id: Id<'a>,
	/// Extensions
	pub extensions: Extensions,
}

impl<'a, T: Clone> Response<'a, T> {
	/// Create a new [`Response`].
	pub fn new(payload: ResponsePayload<'a, T>, id: Id<'a>) -> Response<'a, T> {
		Response { jsonrpc: Some(TwoPointZero), payload, id, extensions: Extensions::new() }
	}

	/// Create a new [`Response`] with extensions
	pub fn new_with_extensions(payload: ResponsePayload<'a, T>, id: Id<'a>, ext: Extensions) -> Response<'a, T> {
		Response { jsonrpc: Some(TwoPointZero), payload, id, extensions: ext }
	}

	/// Create an owned [`Response`].
	pub fn into_owned(self) -> Response<'static, T> {
		Response {
			jsonrpc: self.jsonrpc,
			payload: self.payload.into_owned(),
			id: self.id.into_owned(),
			extensions: self.extensions,
		}
	}

	/// Get the extensions of the response.
	pub fn extensions(&self) -> &Extensions {
		&self.extensions
	}

	/// Get the mutable ref to the extensions of the response.
	pub fn extensions_mut(&mut self) -> &mut Extensions {
		&mut self.extensions
	}
}

impl<T> fmt::Display for Response<'_, T>
where
	T: Serialize + Clone,
{
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&serde_json::to_string(&self).expect("valid JSON; qed"))
	}
}

impl<T> fmt::Debug for Response<'_, T>
where
	T: Serialize + Clone,
{
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&serde_json::to_string(&self).expect("valid JSON; qed"))
	}
}

/// JSON-RPC response object as defined in the [spec](https://www.jsonrpc.org/specification#response_object)
/// but differs from [`Response`] as it only represent a successful response.
#[derive(Debug)]
pub struct Success<'a, T> {
	/// JSON-RPC version.
	pub jsonrpc: Option<TwoPointZero>,
	/// Result.
	pub result: T,
	/// Request ID
	pub id: Id<'a>,
}

impl<'a, T: Clone> TryFrom<Response<'a, T>> for Success<'a, T> {
	type Error = ErrorObjectOwned;

	fn try_from(rp: Response<'a, T>) -> Result<Self, Self::Error> {
		match rp.payload {
			ResponsePayload::Error(e) => Err(e.into_owned()),
			ResponsePayload::Success(r) => Ok(Success { jsonrpc: rp.jsonrpc, result: r.into_owned(), id: rp.id }),
		}
	}
}

/// Return value for subscriptions.
#[derive(Serialize, Deserialize, Debug)]
pub struct SubscriptionPayload<'a, T> {
	/// Subscription ID
	#[serde(borrow)]
	pub subscription: SubscriptionId<'a>,
	/// Result.
	pub result: T,
}

/// Subscription response object, embedding a [`SubscriptionPayload`] in the `params` member along with `result` field.
pub type SubscriptionResponse<'a, T> = Notification<'a, SubscriptionPayload<'a, T>>;
/// Subscription response object, embedding a [`SubscriptionPayload`] in the `params` member along with `error` field.
pub type SubscriptionError<'a, T> = Notification<'a, SubscriptionPayloadError<'a, T>>;

/// Error value for subscriptions.
#[derive(Serialize, Deserialize, Debug)]
pub struct SubscriptionPayloadError<'a, T> {
	/// Subscription ID
	#[serde(borrow)]
	pub subscription: SubscriptionId<'a>,
	/// Result.
	pub error: T,
}

/// Represent the payload of the JSON-RPC response object
///
/// It can be:
///
/// ```json
/// "result":<value>
/// "error":{"code":<code>,"message":<msg>,"data":<data>}
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum ResponsePayload<'a, T>
where
	T: Clone,
{
	/// Corresponds to successful JSON-RPC response with the field `result`.
	Success(StdCow<'a, T>),
	/// Corresponds to failed JSON-RPC response with a error object with the field `error.
	Error(ErrorObject<'a>),
}

impl<'a, T: Clone> ResponsePayload<'a, T> {
	/// Create a successful owned response payload.
	pub fn success(t: T) -> Self {
		Self::Success(StdCow::Owned(t))
	}

	/// Create a successful borrowed response payload.
	pub fn success_borrowed(t: &'a T) -> Self {
		Self::Success(StdCow::Borrowed(t))
	}

	/// Convert the response payload into owned.
	pub fn into_owned(self) -> ResponsePayload<'static, T> {
		match self {
			Self::Error(e) => ResponsePayload::Error(e.into_owned()),
			Self::Success(r) => ResponsePayload::Success(StdCow::Owned(r.into_owned())),
		}
	}

	/// Create an error response payload.
	pub fn error(e: impl Into<ErrorObjectOwned>) -> Self {
		Self::Error(e.into())
	}

	/// Create a borrowed error response payload.
	pub fn error_borrowed(e: impl Into<ErrorObject<'a>>) -> Self {
		Self::Error(e.into())
	}
}

impl<'a, T: Clone> From<ErrorCode> for ResponsePayload<'a, T> {
	fn from(code: ErrorCode) -> ResponsePayload<'a, T> {
		Self::Error(code.into())
	}
}

impl<'de, T> Deserialize<'de> for Response<'de, T>
where
	T: Deserialize<'de> + Clone,
{
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
		T: Deserialize<'de> + Clone,
	{
		#[derive(Debug)]
		enum Field {
			Jsonrpc,
			Result,
			Error,
			Id,
			Ignore,
		}

		impl<'de> Deserialize<'de> for Field {
			fn deserialize<D>(deserializer: D) -> Result<Field, D::Error>
			where
				D: Deserializer<'de>,
			{
				struct FieldVisitor;

				impl serde::de::Visitor<'_> for FieldVisitor {
					type Value = Field;

					fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
						formatter.write_str("`jsonrpc`, `result`, `error` and `id`")
					}

					fn visit_str<E>(self, value: &str) -> Result<Field, E>
					where
						E: serde::de::Error,
					{
						match value {
							"jsonrpc" => Ok(Field::Jsonrpc),
							"result" => Ok(Field::Result),
							"error" => Ok(Field::Error),
							"id" => Ok(Field::Id),
							_ => Ok(Field::Ignore),
						}
					}
				}
				deserializer.deserialize_identifier(FieldVisitor)
			}
		}

		struct Visitor<T>(PhantomData<T>);

		impl<T> Visitor<T> {
			fn new() -> Visitor<T> {
				Visitor(PhantomData)
			}
		}

		impl<'de, T> serde::de::Visitor<'de> for Visitor<T>
		where
			T: Deserialize<'de> + Clone + 'de,
		{
			type Value = Response<'de, T>;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("struct Response")
			}

			fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
			where
				V: serde::de::MapAccess<'de>,
			{
				let mut jsonrpc = None;
				let mut result = None;
				let mut error = None;
				let mut id = None;
				while let Some(key) = map.next_key()? {
					match key {
						Field::Result => {
							if result.is_some() {
								return Err(serde::de::Error::duplicate_field("result"));
							}
							result = Some(map.next_value()?);
						}
						Field::Error => {
							if error.is_some() {
								return Err(serde::de::Error::duplicate_field("error"));
							}
							error = Some(map.next_value()?);
						}
						Field::Id => {
							if id.is_some() {
								return Err(serde::de::Error::duplicate_field("id"));
							}
							id = Some(map.next_value()?);
						}
						Field::Jsonrpc => {
							if jsonrpc.is_some() {
								return Err(serde::de::Error::duplicate_field("jsonrpc"));
							}
							jsonrpc = Some(map.next_value()?);
						}
						Field::Ignore => {
							let _ = map.next_value::<serde::de::IgnoredAny>()?;
						}
					}
				}

				let id = id.ok_or_else(|| serde::de::Error::missing_field("id"))?;

				let response = match (jsonrpc, result, error) {
					(_, Some(_), Some(_)) => {
						return Err(serde::de::Error::duplicate_field("result and error are mutually exclusive"));
					}
					(Some(jsonrpc), Some(result), None) => Response {
						jsonrpc,
						payload: ResponsePayload::Success(result),
						id,
						extensions: Extensions::new(),
					},
					(Some(jsonrpc), None, Some(err)) => {
						Response { jsonrpc, payload: ResponsePayload::Error(err), id, extensions: Extensions::new() }
					}
					(None, Some(result), _) => Response {
						jsonrpc: None,
						payload: ResponsePayload::Success(result),
						id,
						extensions: Extensions::new(),
					},
					(None, _, Some(err)) => Response {
						jsonrpc: None,
						payload: ResponsePayload::Error(err),
						id,
						extensions: Extensions::new(),
					},
					(_, None, None) => return Err(serde::de::Error::missing_field("result/error")),
				};

				Ok(response)
			}
		}

		const FIELDS: &[&str] = &["jsonrpc", "result", "error", "id"];
		deserializer.deserialize_struct("Response", FIELDS, Visitor::new())
	}
}

impl<T> Serialize for Response<'_, T>
where
	T: Serialize + Clone,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		let mut s = serializer.serialize_struct("Response", 3)?;

		if let Some(field) = &self.jsonrpc {
			s.serialize_field("jsonrpc", field)?;
		}

		s.serialize_field("id", &self.id)?;

		match &self.payload {
			ResponsePayload::Error(err) => s.serialize_field("error", err)?,
			ResponsePayload::Success(r) => s.serialize_field("result", r)?,
		};

		s.end()
	}
}

#[cfg(test)]
mod tests {
	use http::Extensions;

	use super::{Id, Response, TwoPointZero};
	use crate::{ErrorObjectOwned, response::ResponsePayload};

	#[test]
	fn serialize_call_ok_response() {
		let ser = serde_json::to_string(&Response {
			jsonrpc: Some(TwoPointZero),
			payload: ResponsePayload::success("ok"),
			id: Id::Number(1),
			extensions: Extensions::new(),
		})
		.unwrap();
		let exp = r#"{"jsonrpc":"2.0","id":1,"result":"ok"}"#;
		assert_eq!(ser, exp);
	}

	#[test]
	fn serialize_call_err_response() {
		let ser = serde_json::to_string(&Response {
			jsonrpc: Some(TwoPointZero),
			payload: ResponsePayload::<()>::error(ErrorObjectOwned::owned(1, "lo", None::<()>)),
			id: Id::Number(1),
			extensions: Extensions::new(),
		})
		.unwrap();
		let exp = r#"{"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"lo"}}"#;
		assert_eq!(ser, exp);
	}

	#[test]
	fn serialize_call_response_missing_version_field() {
		let ser = serde_json::to_string(&Response {
			jsonrpc: None,
			payload: ResponsePayload::success("ok"),
			id: Id::Number(1),
			extensions: Extensions::new(),
		})
		.unwrap();
		let exp = r#"{"id":1,"result":"ok"}"#;
		assert_eq!(ser, exp);
	}

	#[test]
	fn deserialize_success_call() {
		let exp = Response {
			jsonrpc: Some(TwoPointZero),
			payload: ResponsePayload::success(99_u64),
			id: Id::Number(11),
			extensions: Extensions::new(),
		};
		let dsr: Response<u64> = serde_json::from_str(r#"{"jsonrpc":"2.0", "result":99, "id":11}"#).unwrap();
		assert_eq!(dsr.jsonrpc, exp.jsonrpc);
		assert_eq!(dsr.payload, exp.payload);
		assert_eq!(dsr.id, exp.id);
	}

	#[test]
	fn deserialize_err_call() {
		let exp = Response {
			jsonrpc: Some(TwoPointZero),
			payload: ResponsePayload::error(ErrorObjectOwned::owned(1, "lo", None::<()>)),
			id: Id::Number(11),
			extensions: Extensions::new(),
		};
		let dsr: Response<()> =
			serde_json::from_str(r#"{"jsonrpc":"2.0","error":{"code":1,"message":"lo"},"id":11}"#).unwrap();
		assert_eq!(dsr.jsonrpc, exp.jsonrpc);
		assert_eq!(dsr.payload, exp.payload);
		assert_eq!(dsr.id, exp.id);
	}

	#[test]
	fn deserialize_call_missing_version_field() {
		let exp = Response {
			jsonrpc: None,
			payload: ResponsePayload::success(99_u64),
			id: Id::Number(11),
			extensions: Extensions::new(),
		};
		let dsr: Response<u64> = serde_json::from_str(r#"{"jsonrpc":null, "result":99, "id":11}"#).unwrap();
		assert_eq!(dsr.jsonrpc, exp.jsonrpc);
		assert_eq!(dsr.payload, exp.payload);
		assert_eq!(dsr.id, exp.id);
	}

	#[test]
	fn deserialize_with_unknown_field() {
		let exp = Response {
			jsonrpc: None,
			payload: ResponsePayload::success(99_u64),
			id: Id::Number(11),
			extensions: Extensions::new(),
		};
		let dsr: Response<u64> =
			serde_json::from_str(r#"{"jsonrpc":null, "result":99, "id":11, "unknown":11}"#).unwrap();
		assert_eq!(dsr.jsonrpc, exp.jsonrpc);
		assert_eq!(dsr.payload, exp.payload);
		assert_eq!(dsr.id, exp.id);
	}
}
