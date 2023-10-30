use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttestationData {
    pub slot: String,
    pub index: String,
    #[serde(rename = "beacon_block_root")]
    pub beacon_block_root: String,
    pub source: Source,
    pub target: Target,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub epoch: String,
    pub root: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub epoch: String,
    pub root: String,
}
