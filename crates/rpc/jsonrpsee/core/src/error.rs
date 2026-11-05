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

use serde::Serialize;
use serde_json::value::RawValue;

#[derive(Debug, Clone)]
pub(crate) enum InnerSubscriptionErr {
	String(String),
	Json(Box<RawValue>),
}

/// Error returned when a subscription fails where the error is returned
/// as special error notification with the following format:
///
/// ```json
/// {"jsonrpc":"2.0", "method":"subscription_error", "params": {"subscription": "sub_id", "error": <error message from this type>}}
/// ```
///
/// It's recommended to use [`SubscriptionError::from_json`] to create a new instance of this error
/// if the underlying error is a JSON value. That will ensure that the error is serialized correctly.
///
/// SubscriptionError::from will serialize the error as a string, which is not
/// recommended and should only by used in the value of a `String` type.
/// It's mainly provided for convenience and to allow for easy conversion any type that implements StdError.
#[derive(Debug, Clone)]
pub struct SubscriptionError(pub(crate) InnerSubscriptionErr);

impl Serialize for SubscriptionError {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		match &self.0 {
			InnerSubscriptionErr::String(s) => serializer.serialize_str(s),
			InnerSubscriptionErr::Json(json) => json.serialize(serializer),
		}
	}
}

impl<T: ToString> From<T> for SubscriptionError {
	fn from(val: T) -> Self {
		Self(InnerSubscriptionErr::String(val.to_string()))
	}
}

impl SubscriptionError {
	/// Create a new `SubscriptionError` from a JSON value.
	pub fn from_json(json: Box<RawValue>) -> Self {
		Self(InnerSubscriptionErr::Json(json))
	}
}

/// The error returned when registering a method or subscription failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RegisterMethodError {
	/// Method was already registered.
	#[error("Method: {0} was already registered")]
	AlreadyRegistered(String),
	/// Subscribe and unsubscribe method names are the same.
	#[error("Cannot use the same method name for subscribe and unsubscribe, used: {0}")]
	SubscriptionNameConflict(String),
	/// Method with that name has not yet been registered.
	#[error("Method: {0} has not yet been registered")]
	MethodNotFound(String),
}
