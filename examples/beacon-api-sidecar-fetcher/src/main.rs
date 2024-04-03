use serde::{self, Deserialize, Serialize};

fn main() {
    println!("Hello, world!");
}

/// TODO: Import as feature
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobSidecar {
    pub data: Vec<Data>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Data {
    pub index: String,
    pub blob: String,
    #[serde(rename = "kzg_commitment")]
    pub kzg_commitment: String,
    #[serde(rename = "kzg_proof")]
    pub kzg_proof: String,
    #[serde(rename = "signed_block_header")]
    pub signed_block_header: SignedBlockHeader,
    #[serde(rename = "kzg_commitment_inclusion_proof")]
    pub kzg_commitment_inclusion_proof: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedBlockHeader {
    pub message: Message,
    pub signature: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
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
