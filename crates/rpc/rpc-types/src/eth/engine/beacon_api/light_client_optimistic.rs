use serde::{Deserialize, Serialize};
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightClientOptimisticData {
    #[serde(rename = "attested_header")]
    pub attested_header: AttestedHeader,
    #[serde(rename = "sync_aggregate")]
    pub sync_aggregate: SyncAggregate,
    #[serde(rename = "signature_slot")]
    pub signature_slot: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestedHeader {
    pub beacon: Beacon,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Beacon {
    pub slot: String,
    #[serde(rename = "proposer_index")]
    pub proposer_index: String,
    #[serde(rename = "parent_root")]
    pub parent_root: String,
    #[serde(rename = "state_root")]
    pub state_root: String,
    #[serde(rename = "body_root")]
    pub body_root: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAggregate {
    #[serde(rename = "sync_committee_bits")]
    pub sync_committee_bits: String,
    #[serde(rename = "sync_committee_signature")]
    pub sync_committee_signature: String,
}
