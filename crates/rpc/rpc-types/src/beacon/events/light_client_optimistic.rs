use crate::beacon::header::BeaconBlockHeader;
use alloy_primitives::Bytes;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightClientOptimisticData {
    pub attested_header: AttestedHeader,
    pub sync_aggregate: SyncAggregate,
    #[serde_as(as = "DisplayFromStr")]
    pub signature_slot: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedHeader {
    pub beacon: BeaconBlockHeader,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncAggregate {
    pub sync_committee_bits: Bytes,
    pub sync_committee_signature: Bytes,
}
