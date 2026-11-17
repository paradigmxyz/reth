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

//! Shared test helpers for JSONRPC v2.
//!
//! Vendored from [jsonrpsee](https://github.com/paritytech/jsonrpsee), Copyright (c) 2019-2021
//! Parity Technologies (UK) Ltd., licensed under the MIT License (see
//! `crates/rpc/jsonrpsee/LICENSE`).

#![recursion_limit = "256"]

use std::{future::Future, time::Duration};

use tokio::time::{timeout, Timeout};

pub mod helpers;
pub mod mocks;

/// Helper extension trait which allows to limit execution time for the futures.
/// It is helpful in tests to ensure that no future will ever get stuck forever.
pub trait TimeoutFutureExt<T>: Future<Output = T> + Sized {
    /// Returns a reasonable value that can be used as a future timeout with a certain
    /// degree of confidence that timeout won't be triggered by the test specifics.
    fn default_timeout() -> Duration {
        // If some future wasn't done in 60 seconds, it's either a poorly written test
        // or (most likely) a bug related to some future never actually being completed.
        const TIMEOUT_SECONDS: u64 = 60;
        Duration::from_secs(TIMEOUT_SECONDS)
    }

    /// Adds a fixed timeout to the future.
    fn with_default_timeout(self) -> Timeout<Self> {
        self.with_timeout(Self::default_timeout())
    }

    /// Adds a custom timeout to the future.
    fn with_timeout(self, timeout_value: Duration) -> Timeout<Self> {
        timeout(timeout_value, self)
    }
}

impl<T, U> TimeoutFutureExt<T> for U where U: Future<Output = T> + Sized {}
