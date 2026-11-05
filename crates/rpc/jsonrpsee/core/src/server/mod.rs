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

//! Shared modules for the JSON-RPC servers.

/// Error types.
mod error;
/// Helpers.
pub mod helpers;
/// Method response.
mod method_response;
/// JSON-RPC "modules" group sets of methods that belong together and handles method/subscription registration.
mod rpc_module;
/// Subscription related types.
mod subscription;

pub use error::*;
pub use helpers::*;
pub use http::Extensions;
pub use method_response::*;
pub use rpc_module::*;
pub use subscription::*;

use jsonrpsee_types::ErrorObjectOwned;

const LOG_TARGET: &str = "jsonrpsee-server";

/// Something that can be converted into a JSON-RPC method call response.
///
/// If the value couldn't be serialized/encoded, jsonrpsee will sent out an error
/// to the client `response could not be serialized`.
pub trait IntoResponse {
	/// Output.
	type Output: serde::Serialize + Clone;

	/// Something that can be converted into a JSON-RPC method call response.
	fn into_response(self) -> ResponsePayload<'static, Self::Output>;
}

impl<T, E: Into<ErrorObjectOwned>> IntoResponse for Result<T, E>
where
	T: serde::Serialize + Clone,
{
	type Output = T;

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		match self {
			Ok(val) => ResponsePayload::success(val),
			Err(e) => ResponsePayload::error(e),
		}
	}
}

impl<T> IntoResponse for Option<T>
where
	T: serde::Serialize + Clone,
{
	type Output = Option<T>;

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		ResponsePayload::success(self)
	}
}

impl<T> IntoResponse for Vec<T>
where
	T: serde::Serialize + Clone,
{
	type Output = Vec<T>;

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		ResponsePayload::success(self)
	}
}

impl<T, const N: usize> IntoResponse for [T; N]
where
	[T; N]: serde::Serialize + Clone,
{
	type Output = [T; N];

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		ResponsePayload::success(self)
	}
}

impl<T> IntoResponse for jsonrpsee_types::ResponsePayload<'static, T>
where
	T: serde::Serialize + Clone,
{
	type Output = T;

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		self.into()
	}
}

impl<T> IntoResponse for ResponsePayload<'static, T>
where
	T: serde::Serialize + Clone,
{
	type Output = T;

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		self
	}
}

impl IntoResponse for ErrorObjectOwned {
	type Output = ();

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		ResponsePayload::error(self)
	}
}

macro_rules! impl_into_response {
	($($n:ty),*) => {
		$(
			impl IntoResponse for $n {
				type Output = $n;

				fn into_response(self) -> ResponsePayload<'static, Self::Output> {
					ResponsePayload::success(self)
				}
			}
		)+
	}
}

impl_into_response!(
	u8,
	u16,
	u32,
	u64,
	u128,
	usize,
	i8,
	i16,
	i32,
	i64,
	i128,
	isize,
	String,
	&'static str,
	bool,
	serde_json::Value,
	()
);
