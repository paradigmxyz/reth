//! Compatibility functions for rpc proof related types.

use alloy_rpc_types::serde_helpers::JsonStorageKey;
use alloy_rpc_types_eth::{EIP1186AccountProofResponse, EIP1186StorageProof};
use reth_trie_common::{AccountProof, StorageProof};

/// Creates a new rpc storage proof from a primitive storage proof type.
pub fn from_primitive_storage_proof(proof: StorageProof) -> EIP1186StorageProof {
    EIP1186StorageProof { key: JsonStorageKey(proof.key), value: proof.value, proof: proof.proof }
}

/// Creates a new rpc account proof from a primitive account proof type.
pub fn from_primitive_account_proof(proof: AccountProof) -> EIP1186AccountProofResponse {
    let info = proof.info.unwrap_or_default();
    EIP1186AccountProofResponse {
        address: proof.address,
        balance: info.balance,
        code_hash: info.get_bytecode_hash(),
        nonce: info.nonce,
        storage_hash: proof.storage_root,
        account_proof: proof.proof,
        storage_proof: proof.storage_proofs.into_iter().map(from_primitive_storage_proof).collect(),
    }
}
