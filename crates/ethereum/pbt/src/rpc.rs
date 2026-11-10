//! Versioned experimental raw-key proofs over the authenticated RPC transport.

use alloy_primitives::{Bytes, B256};
use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::{
        error::{INTERNAL_ERROR_CODE, INVALID_PARAMS_CODE},
        ErrorObjectOwned,
    },
    RpcModule,
};
use reth_trie_pbt::{persistence::SnapshotStore, ProofTerminal, MAX_KEY_LENGTH};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Authenticated proof service with a bounded number of blocking checkpoint readers.
#[derive(Debug)]
pub(super) struct ProofRpc {
    store: Arc<SnapshotStore>,
    permits: Arc<Semaphore>,
}

impl ProofRpc {
    pub(super) fn new(store: Arc<SnapshotStore>) -> Self {
        Self { store, permits: Arc::new(Semaphore::new(MAX_CONCURRENT_PROOFS)) }
    }

    pub(super) fn into_rpc(self) -> Result<RpcModule<Self>, RegisterMethodError> {
        let mut module = RpcModule::new(self);
        module.register_async_method("pbt_getProof", |params, context, _| async move {
            if params.len_bytes() > MAX_PROOF_PARAMS_BYTES {
                return Err(ErrorObjectOwned::owned(
                    INVALID_PARAMS_CODE,
                    "PBT proof request exceeds the byte limit",
                    None::<()>,
                ));
            }
            let (block_hash, keys) = params.parse::<(B256, Vec<Bytes>)>()?;
            if keys.is_empty() ||
                keys.len() > MAX_PROOF_KEYS ||
                keys.iter().any(|key| key.is_empty() || key.len() > MAX_KEY_LENGTH)
            {
                return Err(ErrorObjectOwned::owned(
                    INVALID_PARAMS_CODE,
                    "PBT proofs require 1..=16 nonempty keys of at most 8192 bytes",
                    None::<()>,
                ));
            }
            let permit = context.permits.clone().try_acquire_owned().map_err(|error| {
                ErrorObjectOwned::owned(
                    LIMIT_EXCEEDED_CODE,
                    "PBT proof service is busy",
                    Some(error.to_string()),
                )
            })?;
            tokio::task::spawn_blocking(move || -> RpcResult<ProofResponse> {
                // Keep the permit until blocking work ends, including if the request is cancelled.
                let _permit = permit;
                let state = context.store.load_block(block_hash).map_err(|error| {
                    ErrorObjectOwned::owned(
                        INTERNAL_ERROR_CODE,
                        "PBT checkpoint could not be loaded",
                        Some(error.to_string()),
                    )
                })?;
                let proofs = keys
                    .into_iter()
                    .map(|key| {
                        let proof = state.tree().proof(&key);
                        let terminal = match proof.terminal {
                            ProofTerminal::Empty => RpcTerminal::Empty,
                            ProofTerminal::Leaf { key, value } => {
                                RpcTerminal::Leaf { key: key.into(), value }
                            }
                            ProofTerminal::Branch { prefix, left, right } => {
                                RpcTerminal::Branch { prefix, left, right }
                            }
                        };
                        KeyProof {
                            value: state.tree().get(&key),
                            key,
                            steps: proof
                                .steps
                                .into_iter()
                                .map(|step| RpcStep {
                                    prefix: step.prefix,
                                    right: step.right,
                                    sibling: step.sibling,
                                })
                                .collect(),
                            terminal,
                        }
                    })
                    .collect();
                Ok(ProofResponse {
                    version: PROOF_VERSION,
                    block_hash,
                    pbt_state_root: state.root(),
                    proofs,
                })
            })
            .await
            .map_err(|error| {
                ErrorObjectOwned::owned(
                    INTERNAL_ERROR_CODE,
                    "PBT proof worker failed",
                    Some(error.to_string()),
                )
            })?
        })?;
        Ok(module)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProofResponse {
    version: u32,
    block_hash: B256,
    pbt_state_root: B256,
    proofs: Vec<KeyProof>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct KeyProof {
    key: Bytes,
    value: Option<B256>,
    steps: Vec<RpcStep>,
    terminal: RpcTerminal,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RpcStep {
    prefix: Vec<bool>,
    right: bool,
    sibling: B256,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum RpcTerminal {
    Empty,
    Leaf { key: Bytes, value: B256 },
    Branch { prefix: Vec<bool>, left: B256, right: B256 },
}

const PROOF_VERSION: u32 = 1;
const MAX_PROOF_KEYS: usize = 16;
const MAX_CONCURRENT_PROOFS: usize = 4;
const MAX_PROOF_PARAMS_BYTES: usize = 2 * MAX_KEY_LENGTH * MAX_PROOF_KEYS + 1024;
const LIMIT_EXCEEDED_CODE: i32 = -32005;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use jsonrpsee::{core::server::MethodsError, rpc_params};
    use reth_trie_pbt::{
        key, migration::DecodeLimits, AccountData, Blake3Hasher, Proof, ProofError, ProofStep,
        StateTree,
    };
    use std::collections::BTreeSet;

    fn verify(proof: KeyProof, root: B256) {
        let terminal = match proof.terminal {
            RpcTerminal::Empty => ProofTerminal::Empty,
            RpcTerminal::Leaf { key, value } => ProofTerminal::Leaf { key: key.to_vec(), value },
            RpcTerminal::Branch { prefix, left, right } => {
                ProofTerminal::Branch { prefix, left, right }
            }
        };
        let witness = Proof {
            steps: proof
                .steps
                .into_iter()
                .map(|step| ProofStep {
                    prefix: step.prefix,
                    right: step.right,
                    sibling: step.sibling,
                })
                .collect(),
            terminal,
        };
        assert_eq!(witness.verify::<Blake3Hasher>(root, &proof.key), Ok(proof.value));
        assert_eq!(
            witness.verify::<Blake3Hasher>(B256::repeat_byte(0xff), &proof.key),
            Err(ProofError::RootMismatch)
        );
    }

    #[tokio::test]
    async fn wire_proofs_authenticate_membership_absence_and_empty_state() {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap());
        let address = Address::repeat_byte(1);
        let mut state = StateTree::new();
        state.set_account(address, AccountData { nonce: 1, ..Default::default() }).unwrap();
        state.set_storage(address, U256::ZERO, B256::repeat_byte(7)).unwrap();
        let block = B256::repeat_byte(1);
        store.save_block(block, &state).unwrap();
        let empty = B256::repeat_byte(2);
        store.save_block(empty, &StateTree::new()).unwrap();
        let module = ProofRpc::new(store).into_rpc().unwrap();
        let keys = vec![
            Bytes::from(key::storage_key::<Blake3Hasher>(address, U256::ZERO)),
            Bytes::from(key::storage_key::<Blake3Hasher>(address, U256::from(1))),
            Bytes::from(vec![0]),
        ];
        let response: ProofResponse =
            module.call("pbt_getProof", rpc_params![block, keys.clone()]).await.unwrap();
        assert_eq!(response.version, PROOF_VERSION);
        assert_eq!(response.block_hash, block);
        assert_eq!(response.pbt_state_root, state.root());
        assert_eq!(response.proofs[0].value, Some(B256::repeat_byte(7)));
        assert_eq!(response.proofs[1].value, None);
        assert!(matches!(response.proofs[2].terminal, RpcTerminal::Branch { .. }));
        let wire = serde_json::to_value(&response).unwrap();
        assert_eq!(wire["proofs"][0]["terminal"]["type"], "leaf");
        assert_eq!(wire["proofs"][2]["terminal"]["type"], "branch");
        let step = wire["proofs"][0]["steps"][0].as_object().unwrap();
        assert_eq!(
            step.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["prefix", "right", "sibling"])
        );
        for (proof, key) in response.proofs.into_iter().zip(keys) {
            assert_eq!(proof.key, key);
            verify(proof, state.root());
        }
        let response: ProofResponse = module
            .call("pbt_getProof", rpc_params![empty, vec![Bytes::from(vec![0])]])
            .await
            .unwrap();
        assert_eq!(response.pbt_state_root, B256::ZERO);
        assert!(matches!(response.proofs[0].terminal, RpcTerminal::Empty));
        verify(response.proofs.into_iter().next().unwrap(), B256::ZERO);
    }

    #[tokio::test]
    async fn invalid_requests_fail_before_reading_a_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap());
        let module = ProofRpc::new(store).into_rpc().unwrap();
        for keys in [
            Vec::new(),
            vec![Bytes::new()],
            vec![Bytes::from(vec![0]); MAX_PROOF_KEYS + 1],
            vec![Bytes::from(vec![0; MAX_KEY_LENGTH + 1])],
        ] {
            let error = module
                .call::<_, ProofResponse>("pbt_getProof", rpc_params![B256::ZERO, keys])
                .await
                .unwrap_err();
            assert!(
                matches!(error, MethodsError::JsonRpc(error) if error.code() == INVALID_PARAMS_CODE)
            );
        }
        let keys = vec![Bytes::from(vec![0; MAX_KEY_LENGTH]); MAX_PROOF_KEYS + 1];
        let error = module
            .call::<_, ProofResponse>("pbt_getProof", rpc_params![B256::ZERO, keys])
            .await
            .unwrap_err();
        assert!(
            matches!(error, MethodsError::JsonRpc(error) if error.code() == INVALID_PARAMS_CODE && error.message() == "PBT proof request exceeds the byte limit")
        );
        let error = module
            .call::<_, ProofResponse>(
                "pbt_getProof",
                rpc_params![B256::ZERO, vec![Bytes::from(vec![0])]],
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, MethodsError::JsonRpc(error) if error.code() == INTERNAL_ERROR_CODE && error.message() == "PBT checkpoint could not be loaded")
        );
    }

    #[tokio::test]
    async fn concurrency_budget_is_released_after_work_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap());
        store.save_block(B256::ZERO, &StateTree::new()).unwrap();
        let service = ProofRpc::new(store);
        let permit =
            service.permits.clone().acquire_many_owned(MAX_CONCURRENT_PROOFS as u32).await.unwrap();
        let module = service.into_rpc().unwrap();
        let error = module
            .call::<_, ProofResponse>(
                "pbt_getProof",
                rpc_params![B256::ZERO, vec![Bytes::from(vec![0])]],
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, MethodsError::JsonRpc(error) if error.code() == LIMIT_EXCEEDED_CODE && error.message() == "PBT proof service is busy")
        );
        drop(permit);
        for _ in 0..2 {
            let response: ProofResponse = module
                .call("pbt_getProof", rpc_params![B256::ZERO, vec![Bytes::from(vec![0])]])
                .await
                .unwrap();
            assert_eq!(response.pbt_state_root, B256::ZERO);
        }
    }

    #[tokio::test]
    async fn maximum_request_and_corrupt_checkpoint_use_the_documented_wire_format() {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap());
        store.save_block(B256::ZERO, &StateTree::new()).unwrap();
        let module = ProofRpc::new(store).into_rpc().unwrap();
        let keys = vec![Bytes::from(vec![0; MAX_KEY_LENGTH]); MAX_PROOF_KEYS];
        let response: ProofResponse =
            module.call("pbt_getProof", rpc_params![B256::ZERO, keys]).await.unwrap();
        assert_eq!(response.proofs.len(), MAX_PROOF_KEYS);
        for proof in response.proofs {
            verify(proof, B256::ZERO);
        }
        let wire: serde_json::Value = module
            .call("pbt_getProof", rpc_params![B256::ZERO, vec![Bytes::from(vec![0])]])
            .await
            .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "version": 1,
                "blockHash": B256::ZERO,
                "pbtStateRoot": B256::ZERO,
                "proofs": [{"key": "0x00", "value": null, "steps": [], "terminal": {"type": "empty"}}],
            })
        );
        drop(module);
        reth_fs_util::write(directory.path().join("states").join(B256::ZERO.to_string()), [0])
            .unwrap();
        let fresh =
            Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap());
        let module = ProofRpc::new(fresh).into_rpc().unwrap();
        let error = module
            .call::<_, ProofResponse>(
                "pbt_getProof",
                rpc_params![B256::ZERO, vec![Bytes::from(vec![0])]],
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, MethodsError::JsonRpc(error) if error.code() == INTERNAL_ERROR_CODE && error.message() == "PBT checkpoint could not be loaded")
        );
    }
}
