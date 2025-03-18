//! RPC validator component used in interop.
//!
//! [`InteropTxValidator`] parses inbox entries from [`AccessListItem`]s, and queries a
//! superchain supervisor for their validity via RPC using the [`CheckAccessList`] API.
// Source: https://github.com/op-rs/kona
// Copyright © 2023 kona contributors Copyright © 2024 Optimism
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
// associated documentation files (the “Software”), to deal in the Software without restriction,
// including without limitation the rights to use, copy, modify, merge, publish, distribute,
// sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all copies or
// substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
// NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
use crate::supervisor::{
    parse_access_list_items_to_inbox_entries, CheckAccessList, ExecutingDescriptor,
    InteropTxValidatorError, SafetyLevel,
};
use alloc::boxed::Box;
use alloy_eips::eip2930::AccessListItem;
use alloy_primitives::B256;
use core::time::Duration;

/// Interacts with a Supervisor to validate inbox entries extracted from [`AccessListItem`]s.
#[async_trait::async_trait]
pub trait InteropTxValidator {
    /// The supervisor client type.
    type SupervisorClient: CheckAccessList + Send + Sync;

    /// Default duration that message validation is not allowed to exceed.
    ///
    /// Note: this has no effect unless shorter than timeout configured for
    /// [`Self::SupervisorClient`] type.
    const DEFAULT_TIMEOUT: Duration;

    /// Returns reference to supervisor client. Used in default trait method implementations.
    fn supervisor_client(&self) -> &Self::SupervisorClient;

    /// Extracts inbox entries from the [`AccessListItem`]s if there are any.
    fn parse_access_list(access_list_items: &[AccessListItem]) -> impl Iterator<Item = &B256> {
        parse_access_list_items_to_inbox_entries(access_list_items.iter())
    }

    /// Validates a list of inbox entries against a Supervisor.
    ///
    /// Times out RPC requests after given timeout, as long as this timeout is shorter
    /// than the underlying request timeout configured for [`Self::SupervisorClient`] type.
    async fn validate_messages_with_timeout(
        &self,
        inbox_entries: &[B256],
        safety: SafetyLevel,
        executing_descriptor: ExecutingDescriptor,
        timeout: Duration,
    ) -> Result<(), InteropTxValidatorError> {
        // Validate messages against supervisor with timeout.
        tokio::time::timeout(
            timeout,
            self.supervisor_client().check_access_list(inbox_entries, safety, executing_descriptor),
        )
        .await
        .map_err(|_| InteropTxValidatorError::ValidationTimeout(timeout.as_secs()))?
    }

    /// Validates a list of inbox entries against a Supervisor.
    ///
    /// Times out RPC requests after [`Self::DEFAULT_TIMEOUT`], as long as this timeout is shorter
    /// than the underlying request timeout configured for [`Self::SupervisorClient`] type.
    async fn validate_messages(
        &self,
        inbox_entries: &[B256],
        safety: SafetyLevel,
        executing_descriptor: ExecutingDescriptor,
    ) -> Result<(), InteropTxValidatorError> {
        self.validate_messages_with_timeout(
            inbox_entries,
            safety,
            executing_descriptor,
            Self::DEFAULT_TIMEOUT,
        )
        .await
    }
}
