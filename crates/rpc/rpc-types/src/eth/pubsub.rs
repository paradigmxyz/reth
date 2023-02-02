//! Ethereum types for pub-sub

use crate::{Log, RichHeader};
use reth_primitives::{rpc::Filter, H256};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

/// Subscription result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SubscriptionResult {
    /// New block header.
    Header(Box<RichHeader>),
    /// Log
    Log(Box<Log>),
    /// Transaction hash
    TransactionHash(H256),
    /// SyncStatus
    SyncState(PubSubSyncStatus),
}

/// Response type for a SyncStatus subscription
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PubSubSyncStatus {
    /// If not currently syncing, this should always be `false`
    Simple(bool),
    /// Current Stats about syncing
    Detailed(SyncStatusMetadata),
}

/// Sync status infos
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct SyncStatusMetadata {
    pub syncing: bool,
    pub starting_block: u64,
    pub current_block: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highest_block: Option<u64>,
}

impl Serialize for SubscriptionResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            SubscriptionResult::Header(ref header) => header.serialize(serializer),
            SubscriptionResult::Log(ref log) => log.serialize(serializer),
            SubscriptionResult::TransactionHash(ref hash) => hash.serialize(serializer),
            SubscriptionResult::SyncState(ref sync) => sync.serialize(serializer),
        }
    }
}

/// Subscription kind.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// New block headers subscription.
    NewHeads,
    /// Logs subscription.
    Logs,
    /// New Pending Transactions subscription.
    NewPendingTransactions,
    /// Node syncing status subscription.
    Syncing,
}

/// Subscription kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Params {
    /// No parameters passed.
    #[default]
    None,
    /// Log parameters.
    Logs(Box<Filter>),
}

impl Serialize for Params {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Params::None => (&[] as &[serde_json::Value]).serialize(serializer),
            Params::Logs(logs) => logs.serialize(serializer),
        }
    }
}

impl<'a> Deserialize<'a> for Params {
    fn deserialize<D>(deserializer: D) -> Result<Params, D::Error>
    where
        D: Deserializer<'a>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;

        if v.is_null() {
            return Ok(Params::None)
        }

        serde_json::from_value(v)
            .map(|f| Params::Logs(Box::new(f)))
            .map_err(|e| D::Error::custom(format!("Invalid Pub-Sub parameters: {e}")))
    }
}
