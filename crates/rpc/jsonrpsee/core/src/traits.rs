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

use jsonrpsee_types::SubscriptionId;
use serde::Serialize;
use serde_json::value::RawValue;

/// Marker trait for types that can be serialized as JSON compatible strings.
///
/// This trait ensures the correctness of the RPC parameters.
///
/// # Note
///
/// Please consider using the [`crate::params::ArrayParams`] and [`crate::params::ObjectParams`] than
/// implementing this trait.
///
/// # Examples
///
/// ## Implementation for hard-coded strings
///
/// ```rust
/// use jsonrpsee_core::traits::ToRpcParams;
/// use serde_json::value::RawValue;
///
/// struct ManualParam;
///
/// impl ToRpcParams for ManualParam {
///     fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
///         // Manually define a valid JSONRPC parameter.
///         serde_json::value::to_raw_value(&[1,2,3]).map(Some)
///     }
/// }
/// ```
///
/// ## Implementation for JSON serializable structures
///
/// ```rust
/// use jsonrpsee_core::traits::ToRpcParams;
/// use serde_json::value::RawValue;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct SerParam {
///     param_1: u8,
///     param_2: String,
/// };
///
/// impl ToRpcParams for SerParam {
///     fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
///         let s = String::from_utf8(serde_json::to_vec(&self)?).expect("Valid UTF8 format");
///         RawValue::from_string(s).map(Some)
///     }
/// }
/// ```
pub trait ToRpcParams {
	/// Consume and serialize the type as a JSON raw value.
	fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error>;
}

// To not bound the `ToRpcParams: Serialize` define a custom implementation
// for types which are serializable.
macro_rules! to_rpc_params_impl {
	() => {
		fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
			let json = serde_json::value::to_raw_value(&self)?;
			Ok(Some(json))
		}
	};
}

impl ToRpcParams for serde_json::Map<String, serde_json::Value> {
	to_rpc_params_impl!();
}

impl<P: Serialize> ToRpcParams for &[P] {
	to_rpc_params_impl!();
}

impl<P: Serialize> ToRpcParams for Vec<P> {
	to_rpc_params_impl!();
}
impl<P, const N: usize> ToRpcParams for [P; N]
where
	[P; N]: Serialize,
{
	to_rpc_params_impl!();
}

macro_rules! tuple_impls {
	($($len:expr => ($($n:tt $name:ident)+))+) => {
		$(
			impl<$($name: Serialize),+> ToRpcParams for ($($name,)+) {
				to_rpc_params_impl!();
			}
		)+
	}
}

tuple_impls! {
	1 => (0 T0)
	2 => (0 T0 1 T1)
	3 => (0 T0 1 T1 2 T2)
	4 => (0 T0 1 T1 2 T2 3 T3)
	5 => (0 T0 1 T1 2 T2 3 T3 4 T4)
	6 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5)
	7 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6)
	8 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7)
	9 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8)
	10 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9)
	11 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10)
	12 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11)
	13 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12)
	14 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13)
	15 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14)
	16 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14 15 T15)
}

/// Trait to generate subscription IDs.
pub trait IdProvider: Send + Sync + std::fmt::Debug {
	/// Returns the next ID for the subscription.
	fn next_id(&self) -> SubscriptionId<'static>;
}

// Implement `IdProvider` for `Box<T>`
//
// It's not implemented for `&'_ T` because
// of the required `'static lifetime`
// Thus, `&dyn IdProvider` won't work.
impl<T: IdProvider + ?Sized> IdProvider for Box<T> {
	fn next_id(&self) -> SubscriptionId<'static> {
		(**self).next_id()
	}
}

/// Interface for types that can be serialized into JSON.
pub trait ToJson {
	/// Convert the type into a JSON value.
	fn to_json(&self) -> Result<Box<RawValue>, serde_json::Error>;
}
