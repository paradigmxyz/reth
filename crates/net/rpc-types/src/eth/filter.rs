use crate::Log;
use reth_primitives::H256;
pub use reth_primitives::{Filter, FilteredParams};
use serde::{Serialize, Serializer};

/// Response of the `eth_getFilterChanges` RPC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterChanges {
    /// New logs.
    Logs(Vec<Log>),
    /// New hashes (block or transactions)
    Hashes(Vec<H256>),
    /// Empty result,
    Empty,
}

impl Serialize for FilterChanges {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            FilterChanges::Logs(logs) => logs.serialize(s),
            FilterChanges::Hashes(hashes) => hashes.serialize(s),
            FilterChanges::Empty => (&[] as &[serde_json::Value]).serialize(s),
        }
    }
}
