//! MPT hash builder implementation.

mod state;
pub use state::HashBuilderState;

mod value;
pub(crate) use value::StoredHashBuilderValue;

pub use alloy_trie::hash_builder::*;
