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

//! RPC parameters.

use crate::traits::ToRpcParams;
use serde::Serialize;
use serde_json::value::RawValue;

/// Helper module for building parameters.
mod params_builder {
	use serde::Serialize;
	use serde_json::value::RawValue;

	/// Initial number of bytes for a parameter length.
	const PARAM_BYTES_CAPACITY: usize = 128;

	/// Generic parameter builder that serializes parameters to bytes.
	/// This produces a JSON compatible String.
	///
	/// The implementation relies on `Vec<u8>` to hold the serialized
	/// parameters in memory for the following reasons:
	///   1. Other serialization methods than `serde_json::to_writer` would internally
	///      have an extra heap allocation for temporarily holding the value in memory.
	///   2. `io::Write` is not implemented for `String` required for serialization.
	#[derive(Debug, Clone)]
	pub(crate) struct ParamsBuilder {
		bytes: Vec<u8>,
		start: char,
		end: char,
	}

	impl ParamsBuilder {
		/// Construct a new [`ParamsBuilder`] with custom start and end tokens.
		/// The inserted values are wrapped by the _start_ and _end_ tokens.
		fn new(start: char, end: char) -> Self {
			ParamsBuilder { bytes: Vec::new(), start, end }
		}

		/// Construct a new [`ParamsBuilder`] for positional parameters equivalent to a JSON array object.
		pub(crate) fn positional() -> Self {
			Self::new('[', ']')
		}

		/// Construct a new [`ParamsBuilder`] for named parameters equivalent to a JSON map object.
		pub(crate) fn named() -> Self {
			Self::new('{', '}')
		}

		/// Initialize the internal vector if it is empty:
		///  - allocate [`PARAM_BYTES_CAPACITY`] to avoid resizing
		///  - add the `start` character.
		///
		/// # Note
		///
		/// Initialization is needed prior to inserting elements.
		fn maybe_initialize(&mut self) {
			if self.bytes.is_empty() {
				self.bytes.reserve(PARAM_BYTES_CAPACITY);
				self.bytes.push(self.start as u8);
			}
		}

		/// Insert a named value (key, value) pair into the builder.
		/// The _name_ and _value_ are delimited by the `:` token.
		pub(crate) fn insert_named<P: Serialize>(&mut self, name: &str, value: P) -> Result<(), serde_json::Error> {
			self.maybe_initialize();

			serde_json::to_writer(&mut self.bytes, name)?;
			self.bytes.push(b':');
			serde_json::to_writer(&mut self.bytes, &value)?;
			self.bytes.push(b',');

			Ok(())
		}

		/// Insert a plain value into the builder.
		pub(crate) fn insert<P: Serialize>(&mut self, value: P) -> Result<(), serde_json::Error> {
			self.maybe_initialize();

			serde_json::to_writer(&mut self.bytes, &value)?;
			self.bytes.push(b',');

			Ok(())
		}

		/// Finish the building process and return a JSON object.
		pub(crate) fn build(mut self) -> Option<Box<RawValue>> {
			if self.bytes.is_empty() {
				return None;
			}

			let idx = self.bytes.len() - 1;
			if self.bytes[idx] == b',' {
				self.bytes[idx] = self.end as u8;
			} else {
				self.bytes.push(self.end as u8);
			}

			// Safety: This is safe because JSON does not emit invalid UTF-8.
			let json_str = unsafe { String::from_utf8_unchecked(self.bytes) };
			Some(RawValue::from_string(json_str).expect("Valid JSON String; qed"))
		}
	}
}

/// Parameter builder that serializes named value parameters to a JSON compatible string.
/// This is the equivalent of a JSON Map object `{ key: value }`.
///
/// # Examples
///
/// ```rust
///
/// use jsonrpsee_core::params::ObjectParams;
///
/// let mut builder = ObjectParams::new();
/// builder.insert("param1", 1);
/// builder.insert("param2", "abc");
///
/// // Use RPC parameters...
/// ```
#[derive(Debug, Clone)]
pub struct ObjectParams(params_builder::ParamsBuilder);

impl ObjectParams {
	/// Construct a new [`ObjectParams`].
	pub fn new() -> Self {
		Self::default()
	}

	/// Insert a named value (key, value) pair into the builder.
	/// The _name_ and _value_ are delimited by the `:` token.
	pub fn insert<P: Serialize>(&mut self, name: &str, value: P) -> Result<(), serde_json::Error> {
		self.0.insert_named(name, value)
	}
}

impl Default for ObjectParams {
	fn default() -> Self {
		Self(params_builder::ParamsBuilder::named())
	}
}

impl ToRpcParams for ObjectParams {
	fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
		Ok(self.0.build())
	}
}

/// Parameter builder that serializes plain value parameters to a JSON compatible string.
/// This is the equivalent of a JSON Array object `[ value0, value1, .., valueN ]`.
///
/// # Examples
///
/// ```rust
///
/// use jsonrpsee_core::params::ArrayParams;
///
/// let mut builder = ArrayParams::new();
/// builder.insert("param1");
/// builder.insert(1);
///
/// // Use RPC parameters...
/// ```
#[derive(Debug, Clone)]
pub struct ArrayParams(params_builder::ParamsBuilder);

impl ArrayParams {
	/// Construct a new [`ArrayParams`].
	pub fn new() -> Self {
		Self::default()
	}

	/// Insert a plain value into the builder.
	pub fn insert<P: Serialize>(&mut self, value: P) -> Result<(), serde_json::Error> {
		self.0.insert(value)
	}
}

impl Default for ArrayParams {
	fn default() -> Self {
		Self(params_builder::ParamsBuilder::positional())
	}
}

impl ToRpcParams for ArrayParams {
	fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
		Ok(self.0.build())
	}
}

/// Initial number of parameters in a batch request.
const BATCH_PARAMS_NUM_CAPACITY: usize = 4;

/// Error representing an empty batch request.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("Empty batch request is not allowed")]
pub struct EmptyBatchRequest;

/// Request builder that serializes RPC parameters to construct a valid batch parameter.
/// This is the equivalent of chaining multiple RPC requests.
#[derive(Clone, Debug, Default)]
pub struct BatchRequestBuilder<'a>(Vec<(&'a str, Option<Box<RawValue>>)>);

impl<'a> BatchRequestBuilder<'a> {
	/// Construct a new [`BatchRequestBuilder`].
	pub fn new() -> Self {
		Self(Vec::with_capacity(BATCH_PARAMS_NUM_CAPACITY))
	}

	/// Inserts the RPC method with provided parameters into the builder.
	pub fn insert<Params: ToRpcParams>(&mut self, method: &'a str, value: Params) -> Result<(), serde_json::Error> {
		self.0.push((method, value.to_rpc_params()?));
		Ok(())
	}

	/// Finish the building process and return a valid batch parameter.
	#[allow(clippy::type_complexity)]
	pub fn build(self) -> Result<Vec<(&'a str, Option<Box<RawValue>>)>, EmptyBatchRequest> {
		if self.0.is_empty() { Err(EmptyBatchRequest) } else { Ok(self.0) }
	}

	/// Get an iterator over the batch request.
	pub fn iter(&self) -> impl Iterator<Item = (&'a str, Option<&RawValue>)> {
		self.0.iter().map(|(method, params)| (*method, params.as_deref()))
	}
}

impl<'a> IntoIterator for BatchRequestBuilder<'a> {
	type Item = (&'a str, Option<Box<RawValue>>);
	type IntoIter = std::vec::IntoIter<Self::Item>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.into_iter()
	}
}
