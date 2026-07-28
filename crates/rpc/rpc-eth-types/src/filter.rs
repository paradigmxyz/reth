//! Types used by the Ethereum filter RPC methods.

use alloy_primitives::B256;
use alloy_rpc_types_eth::{Log, Transaction};
use serde::{Deserialize, Deserializer, Serialize};

/// Response of the `eth_getFilterChanges` RPC.
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum FilterChanges<T = Transaction, L = Log> {
    /// Empty result.
    #[serde(with = "empty_array")]
    #[default]
    Empty,
    /// New logs.
    Logs(Vec<L>),
    /// New hashes (block or transactions).
    Hashes(Vec<B256>),
    /// New transactions.
    Transactions(Vec<T>),
}

impl From<Vec<Log>> for FilterChanges {
    fn from(logs: Vec<Log>) -> Self {
        Self::Logs(logs)
    }
}

impl From<Vec<B256>> for FilterChanges {
    fn from(hashes: Vec<B256>) -> Self {
        Self::Hashes(hashes)
    }
}

impl From<Vec<Transaction>> for FilterChanges {
    fn from(transactions: Vec<Transaction>) -> Self {
        Self::Transactions(transactions)
    }
}

impl<T, L> FilterChanges<T, L> {
    /// Returns the hashes if present.
    pub fn as_hashes(&self) -> Option<&[B256]> {
        if let Self::Hashes(hashes) = self {
            Some(hashes)
        } else {
            None
        }
    }

    /// Returns the logs if present.
    pub fn as_logs(&self) -> Option<&[L]> {
        if let Self::Logs(logs) = self {
            Some(logs)
        } else {
            None
        }
    }

    /// Returns the transactions if present.
    pub fn as_transactions(&self) -> Option<&[T]> {
        if let Self::Transactions(transactions) = self {
            Some(transactions)
        } else {
            None
        }
    }

    /// Returns whether this is an empty response.
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Returns whether this response contains logs.
    pub const fn is_logs(&self) -> bool {
        matches!(self, Self::Logs(_))
    }

    /// Returns whether this response contains hashes.
    pub const fn is_hashes(&self) -> bool {
        matches!(self, Self::Hashes(_))
    }

    /// Returns whether this response contains transactions.
    pub const fn is_transactions(&self) -> bool {
        matches!(self, Self::Transactions(_))
    }
}

mod empty_array {
    use serde::{Serialize, Serializer};

    pub(super) fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        (&[] as &[()]).serialize(serializer)
    }
}

impl<'de, T, L> Deserialize<'de> for FilterChanges<T, L>
where
    T: Deserialize<'de>,
    L: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Changes<T, L> {
            Hashes(Vec<B256>),
            Logs(Vec<L>),
            Transactions(Vec<T>),
        }

        Ok(match Changes::<T, L>::deserialize(deserializer)? {
            Changes::Hashes(values) if values.is_empty() => Self::Empty,
            Changes::Logs(values) if values.is_empty() => Self::Empty,
            Changes::Transactions(values) if values.is_empty() => Self::Empty,
            Changes::Hashes(values) => Self::Hashes(values),
            Changes::Logs(values) => Self::Logs(values),
            Changes::Transactions(values) => Self::Transactions(values),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct CustomLog {
        value: u64,
    }

    #[test]
    fn custom_log_roundtrip() {
        let changes = FilterChanges::<Transaction, CustomLog>::Logs(vec![CustomLog { value: 42 }]);
        let value = serde_json::to_value(&changes).unwrap();
        assert_eq!(value, serde_json::json!([{ "value": 42 }]));
        assert_eq!(
            serde_json::from_value::<FilterChanges<Transaction, CustomLog>>(value).unwrap(),
            changes
        );
    }

    #[test]
    fn empty_roundtrip() {
        let changes = FilterChanges::<Transaction, CustomLog>::Empty;
        let value = serde_json::to_value(&changes).unwrap();
        assert_eq!(value, serde_json::json!([]));
        assert_eq!(
            serde_json::from_value::<FilterChanges<Transaction, CustomLog>>(value).unwrap(),
            changes
        );
    }
}
