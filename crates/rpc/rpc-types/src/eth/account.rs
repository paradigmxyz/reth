#![allow(missing_docs)]
use reth_primitives::{Address, Bytes, H256, H512, U256, U64};
use serde::{Deserialize, Serialize};

/// Account information.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Account name
    pub name: String,
}

/// Data structure with proof for one single storage-entry
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageProof {
    /// Storage key.
    pub key: U256,
    /// Value that the key holds
    pub value: U256,
    /// proof for the pair
    pub proof: Vec<Bytes>,
}

/// Response for EIP-1186 account proof `eth_getProof`
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EIP1186AccountProofResponse {
    pub address: Address,
    pub balance: U256,
    pub code_hash: H256,
    pub nonce: U64,
    pub storage_hash: H256,
    pub account_proof: Vec<Bytes>,
    pub storage_proof: Vec<StorageProof>,
}

/// Extended account information (used by `parity_allAccountInfo`).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtAccountInfo {
    /// Account name
    pub name: String,
    /// Account meta JSON
    pub meta: String,
    /// Account UUID (`None` for address book entries)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}

/// account derived from a signature
/// as well as information that tells if it is valid for
/// the current chain
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredAccount {
    /// address of the recovered account
    pub address: Address,
    /// public key of the recovered account
    pub public_key: H512,
    /// If the signature contains chain replay protection,
    /// And the chain_id encoded within the signature
    /// matches the current chain this would be true, otherwise false.
    pub is_valid_for_current_chain: bool,
}
