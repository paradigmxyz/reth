use serde::{Deserialize, Serialize};
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightClientOptimisticData {
    pub attested_header: AttestedHeader,
    pub sync_aggregate: SyncAggregate,
    pub signature_slot: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttestedHeader {
    pub beacon: Beacon,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Beacon {
    pub slot: String,
    pub proposer_index: String,
    pub parent_root: String,
    pub state_root: String,
    pub body_root: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncAggregate {
    pub sync_committee_bits: String,
    pub sync_committee_signature: String,
}
