//! Copied interop related kona code
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
// TODO: we should remove everything expect for supervisor once kona could be used as dependency in
// reth
mod access_list;
pub use access_list::parse_access_list_items_to_inbox_entries;
mod api;
pub use api::CheckAccessList;
mod safety;
pub use safety::SafetyLevel;
// TODO: this should be renamed to supervisor once we remove kona from here
mod reth_supervisor;
pub use reth_supervisor::{SupervisorClient, DEFAULT_SUPERVISOR_URL};
mod constants;
pub use constants::CROSS_L2_INBOX_ADDRESS;
mod errors;
pub use errors::{InteropTxValidatorError, InvalidInboxEntry};
mod message;
pub use message::ExecutingDescriptor;
mod reqwest;
pub use reqwest::SupervisorClient as ReqwestSupervisorClient;

mod traits;
pub use traits::InteropTxValidator;
