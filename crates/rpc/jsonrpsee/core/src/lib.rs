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

//! Shared utilities for `jsonrpsee`.
//!
//! Vendored from [jsonrpsee](https://github.com/paritytech/jsonrpsee), Copyright (c) 2019-2021
//! Parity Technologies (UK) Ltd., licensed under the MIT License (see
//! `crates/rpc/jsonrpsee/LICENSE`).

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

/// Error type.
pub mod error;

/// Traits
pub mod traits;

#[cfg(feature = "http-helpers")]
pub mod http_helpers;

#[cfg(feature = "server")]
pub mod id_providers;

#[cfg(feature = "server")]
pub mod middleware;

#[cfg(feature = "server")]
pub mod server;

pub use error::{RegisterMethodError, SubscriptionError};

/// JSON-RPC result.
pub type RpcResult<T> = std::result::Result<T, jsonrpsee_types::ErrorObjectOwned>;

#[doc(hidden)]
mod proc_macros_support;

/// Re-exports for proc-macro library to not require any additional
/// dependencies to be explicitly added by users of the macro.
#[doc(hidden)]
pub mod __reexports {
    // Needed for the params parsing in the proc macro API.
    #[cfg(feature = "server")]
    pub use tokio;

    pub use super::proc_macros_support::*;
}

pub use serde::{de::DeserializeOwned, Serialize};
pub use serde_json::{
    value::{to_raw_value as to_json_raw_value, RawValue as JsonRawValue},
    Value as JsonValue,
};

/// Ten megabytes.
pub const TEN_MB_SIZE_BYTES: u32 = 10 * 1024 * 1024;

/// The return type if the subscription wants to return `Result`.
pub type SubscriptionResult = Result<(), SubscriptionError>;

/// Type erased error.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Skips the leading whitespace of a JSON-RPC message and tells a single request from a batch by
/// its first byte.
///
/// Returns the message from that byte on and whether it is a single request, or `None` if the
/// first 128 bytes do not lead to a `{` or a `[`. Only the four JSON whitespace characters are
/// skipped, not for example form feed.
pub fn classify_message(data: &[u8]) -> Option<(&[u8], bool)> {
    let start =
        data.iter().take(128).position(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))?;

    match data[start] {
        b'{' => Some((&data[start..], true)),
        b'[' => Some((&data[start..], false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::classify_message;
    use test_case::test_case;

    #[test_case(b"{}", Some((b"{}", true)); "single")]
    #[test_case(b"[]", Some((b"[]", false)); "batch")]
    #[test_case(b" \t\r\n{}", Some((b"{}", true)); "json whitespace is skipped")]
    #[test_case(b"\x0c{}", None; "form feed is not whitespace")]
    #[test_case(b"1", None; "neither an object nor an array")]
    #[test_case(b"", None; "empty")]
    #[test_case(b"   ", None; "only whitespace")]
    #[test_case(&[[b' '; 127].as_slice(), b"[]"].concat(), Some((b"[]", false)); "last byte looked at")]
    #[test_case(&[[b' '; 128].as_slice(), b"[]"].concat(), None; "past the bytes looked at")]
    fn classify_message_cases(data: &[u8], expected: Option<(&[u8], bool)>) {
        assert_eq!(classify_message(data), expected);
    }
}
