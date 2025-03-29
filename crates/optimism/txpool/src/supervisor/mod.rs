//! Supervisor support for interop
mod access_list;
pub use access_list::parse_access_list_items_to_inbox_entries;
pub use op_alloy_consensus::interop::*;

mod client;
pub use client::{SupervisorClient, DEFAULT_SUPERVISOR_URL};
mod errors;
pub use errors::{InteropTxValidatorError, InvalidInboxEntry};
mod message;
pub use message::ExecutingDescriptor;
mod validator;
pub use validator::is_valid_cross_tx;
