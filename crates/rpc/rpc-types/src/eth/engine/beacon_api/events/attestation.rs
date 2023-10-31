use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationData {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub index: u64,
    pub beacon_block_root: B256,
    pub source: Source,
    pub target: Target,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    #[serde_as(as = "DisplayFromStr")]
    pub epoch: u64,
    pub root: B256,
}
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde_as(as = "DisplayFromStr")]
    pub epoch: u64,
    pub root: B256,
}
