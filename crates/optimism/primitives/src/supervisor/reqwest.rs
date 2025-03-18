//! RPC API implementation using `reqwest`
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
    api::CheckAccessList, ExecutingDescriptor, InteropTxValidatorError, SafetyLevel,
};
use alloy_primitives::B256;
use alloy_rpc_client::ReqwestClient;
use derive_more::Constructor;

/// A supervisor client.
#[derive(Debug, Clone, Constructor)]
pub struct SupervisorClient {
    /// The inner RPC client.
    client: ReqwestClient,
}

impl CheckAccessList for SupervisorClient {
    async fn check_access_list(
        &self,
        inbox_entries: &[B256],
        min_safety: SafetyLevel,
        executing_descriptor: ExecutingDescriptor,
    ) -> Result<(), InteropTxValidatorError> {
        self.client
            .request(
                "supervisor_checkAccessList",
                (inbox_entries, min_safety, executing_descriptor),
            )
            .await
            .map_err(InteropTxValidatorError::client)
    }
}
