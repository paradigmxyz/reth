//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-sidecar-fetcher -- node

use serde::{self, Deserialize, Serialize};

fn main() {
    // Example JSON string (replace this with your actual JSON string)
    let json_str = r#"
    {
        "code": 400,
        "message": "Invalid block ID: current"
    }
    "#;

    // Deserialize the JSON string into the BlobError struct
    let parsed: Result<BlobError, serde_json::Error> = serde_json::from_str(json_str);

    match parsed {
        Ok(blob_error) => println!("{:?}: {}", blob_error.code, blob_error.message),
        Err(e) => println!("Failed to parse JSON: {}", e),
    }
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

#[derive(Serialize, Deserialize, Debug)]
struct BlobError {
    #[serde(rename = "code")]
    code: u16,
    #[serde(rename = "message")]
    message: String,
}
