use crate::{
    selection::{encoded_size, remaining_materialization, select_with_source, source_budget},
    verify_receipt_selection, ReceiptSelection, SelectionError, SelectionLimits,
    SelectionOperation, SelectionProofFormat, SelectionRequest, SelectionResponse,
};
use alloy_primitives::B256;
use reth_pureth_receipt::{ProviderSnapshot, RootContext};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{
    convert_eip6466_block_receipts, convert_eip6466_receipt, verify_eip6466_receipt_response,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WholeReceiptProof {
    pub block_hash: B256,
    pub receipt_index: u64,
    pub root_context: String,
    pub selection: SelectionResponse,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum ReceiptResponse<T> {
    Legacy(T),
    WithProof {
        #[serde(flatten)]
        receipt: T,
        #[serde(rename = "sszProof")]
        ssz_proof: Option<WholeReceiptProof>,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ReceiptProofSupport<'a> {
    Supported(&'a ProviderSnapshot),
    Unsupported,
}

fn request(indices: impl IntoIterator<Item = u64>) -> SelectionRequest {
    SelectionRequest {
        include_proof: true,
        selections: indices
            .into_iter()
            .map(|index| ReceiptSelection {
                path: format!("[{index}]"),
                operation: SelectionOperation::Whole {},
            })
            .collect(),
    }
}

fn proof(
    snapshot: &ProviderSnapshot,
    index: u64,
    selection: SelectionResponse,
) -> WholeReceiptProof {
    WholeReceiptProof {
        block_hash: snapshot.block_hash(),
        receipt_index: index,
        root_context: match snapshot.root_context() {
            RootContext::DeterministicTestData => "deterministic_test_data",
            RootContext::RethExperimentalUnanchored => "reth_experimental_unanchored",
        }
        .into(),
        selection,
    }
}

fn check_receipt(receipt: &impl Serialize) -> Result<(), SelectionError> {
    let value = serde_json::to_value(receipt).map_err(|_| SelectionError::InvalidValue)?;
    if value.as_object().is_none_or(|object| object.contains_key("sszProof")) {
        return Err(SelectionError::InvalidContext);
    }
    Ok(())
}

pub fn extend_receipt_response<T: Serialize>(
    receipt: T,
    index: u64,
    support: ReceiptProofSupport<'_>,
    enabled: bool,
    limits: SelectionLimits,
) -> Result<ReceiptResponse<T>, SelectionError> {
    if !enabled {
        return Ok(ReceiptResponse::Legacy(receipt));
    }
    encoded_size(&receipt, limits.response_bytes, "response bytes")?;
    check_receipt(&receipt)?;
    let ssz_proof = match support {
        ReceiptProofSupport::Unsupported => None,
        ReceiptProofSupport::Supported(snapshot) => {
            let selection = crate::select_receipt_snapshot(
                snapshot.receipt_snapshot(),
                &request([index]),
                limits,
            )?;
            Some(proof(snapshot, index, selection))
        }
    };
    let response = ReceiptResponse::WithProof { receipt, ssz_proof };
    encoded_size(&response, limits.response_bytes, "response bytes")?;
    Ok(response)
}

pub fn extend_block_receipt_responses<T: Serialize>(
    receipts: Vec<T>,
    support: ReceiptProofSupport<'_>,
    enabled: bool,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<Vec<ReceiptResponse<T>>, SelectionError> {
    if !enabled {
        return Ok(receipts.into_iter().map(ReceiptResponse::Legacy).collect());
    }
    encoded_size(&receipts, limits.response_bytes, "response bytes")?;
    for receipt in &receipts {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        check_receipt(receipt)?;
    }
    let mut proofs = Vec::new();
    if let ReceiptProofSupport::Supported(snapshot) = support {
        let retained = snapshot.receipt_snapshot();
        if retained.receipts().len() != receipts.len() {
            return Err(SelectionError::InvalidContext);
        }
        let source = source_budget(retained, limits, cancelled)?;
        if receipts.len() > limits.targets {
            return Err(SelectionError::LimitExceeded("targets"));
        }
        if limits.selections == 0 {
            return Err(SelectionError::LimitExceeded("selections"));
        }
        let mut value_bytes = 0;
        let mut hashes = 0;
        for start in (0..receipts.len()).step_by(limits.selections) {
            if cancelled.load(Ordering::Relaxed) {
                return Err(SelectionError::Cancelled);
            }
            let end = receipts.len().min(start.saturating_add(limits.selections));
            let selection = select_with_source(
                retained,
                &request(start as u64..end as u64),
                limits,
                cancelled,
                SelectionProofFormat::SingleBranch,
                source,
            )?;
            for (index, result) in (start..end).zip(selection.results) {
                for value in &result.values_ssz {
                    crate::selection::add(
                        &mut value_bytes,
                        value.len(),
                        limits.response_bytes,
                        "value bytes",
                    )?;
                }
                for witness in result.witnesses.as_deref().ok_or(SelectionError::InvalidProof)? {
                    crate::selection::add(
                        &mut hashes,
                        witness.branch.len() + 1,
                        limits.witness_hashes,
                        "witness hashes",
                    )?;
                }
                remaining_materialization(source, value_bytes, hashes, limits.source_bytes)?;
                proofs.push(Some(proof(
                    snapshot,
                    index as u64,
                    SelectionResponse {
                        root: retained.root(),
                        proof_format: Some(SelectionProofFormat::SingleBranch),
                        results: vec![result],
                        multiproof: None,
                    },
                )));
            }
        }
    } else {
        proofs.resize(receipts.len(), None);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    let responses = receipts
        .into_iter()
        .zip(proofs)
        .map(|(receipt, ssz_proof)| ReceiptResponse::WithProof { receipt, ssz_proof })
        .collect::<Vec<_>>();
    encoded_size(&responses, limits.response_bytes, "response bytes")?;
    Ok(responses)
}

pub fn verify_whole_receipt_proof(
    block_hash: B256,
    index: u64,
    proof: &WholeReceiptProof,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    encoded_size(proof, limits.response_bytes, "response bytes")?;
    if proof.block_hash != block_hash ||
        proof.receipt_index != index ||
        !matches!(
            proof.root_context.as_str(),
            "deterministic_test_data" | "reth_experimental_unanchored"
        )
    {
        return Err(SelectionError::InvalidContext);
    }
    verify_receipt_selection(&request([index]), &proof.selection, expected_root, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_pureth_receipt::{
        DeterministicProvider, ObjectKind, PROGRESSIVE_RECEIPTS_BLOCK_HASH, SINGLETON_BLOCK_HASH,
    };
    #[test]
    fn receipt_response_preserves_legacy_and_distinguishes_null_from_errors() {
        let receipt = serde_json::json!({"status": "0x1", "transactionHash": B256::ZERO});
        let legacy = extend_receipt_response(
            receipt.clone(),
            0,
            ReceiptProofSupport::Unsupported,
            false,
            SelectionLimits::default(),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), receipt);
        let unsupported = extend_receipt_response(
            receipt.clone(),
            0,
            ReceiptProofSupport::Unsupported,
            true,
            SelectionLimits::default(),
        )
        .unwrap();
        assert!(serde_json::to_value(unsupported).unwrap()["sszProof"].is_null());
        let provider = DeterministicProvider::new().unwrap();
        let snapshot = provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Receipts).unwrap();
        let supported = extend_receipt_response(
            receipt.clone(),
            0,
            ReceiptProofSupport::Supported(snapshot),
            true,
            SelectionLimits::default(),
        )
        .unwrap();
        let ReceiptResponse::WithProof { ssz_proof: Some(proof), .. } = supported else {
            panic!("expected proof");
        };
        verify_whole_receipt_proof(
            snapshot.block_hash(),
            0,
            &proof,
            snapshot.root(),
            SelectionLimits::default(),
        )
        .unwrap();
        assert!(verify_whole_receipt_proof(
            B256::ZERO,
            0,
            &proof,
            snapshot.root(),
            SelectionLimits::default()
        )
        .is_err());
        assert!(extend_receipt_response(
            receipt,
            1,
            ReceiptProofSupport::Supported(snapshot),
            true,
            SelectionLimits::default()
        )
        .is_err());
    }
    #[test]
    fn block_receipt_proofs_share_the_snapshot_but_verify_each_complete_element() {
        let provider = DeterministicProvider::new().unwrap();
        let snapshot =
            provider.lookup(PROGRESSIVE_RECEIPTS_BLOCK_HASH, ObjectKind::Receipts).unwrap();
        let receipts = (0..snapshot.receipt_snapshot().receipts().len())
            .map(|index| serde_json::json!({"transactionIndex": index}))
            .collect();
        let responses = extend_block_receipt_responses(
            receipts,
            ReceiptProofSupport::Supported(snapshot),
            true,
            SelectionLimits { selections: 2, ..Default::default() },
            &AtomicBool::new(false),
        )
        .unwrap();
        for (index, response) in responses.iter().enumerate() {
            let ReceiptResponse::WithProof { ssz_proof: Some(proof), .. } = response else {
                panic!("expected proof");
            };
            verify_whole_receipt_proof(
                snapshot.block_hash(),
                index as u64,
                proof,
                snapshot.root(),
                SelectionLimits::default(),
            )
            .unwrap();
        }
        assert!(extend_block_receipt_responses(
            vec![serde_json::json!({})],
            ReceiptProofSupport::Supported(snapshot),
            true,
            SelectionLimits::default(),
            &AtomicBool::new(false)
        )
        .is_err());
        let receipts = (0..snapshot.receipt_snapshot().receipts().len())
            .map(|_| serde_json::json!({}))
            .collect();
        assert!(extend_block_receipt_responses(
            receipts,
            ReceiptProofSupport::Supported(snapshot),
            true,
            SelectionLimits::default(),
            &AtomicBool::new(true)
        )
        .is_err());
    }

    #[test]
    fn whole_receipt_verification_caps_the_complete_proof_envelope() {
        let provider = DeterministicProvider::new().unwrap();
        let snapshot = provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Receipts).unwrap();
        let response = extend_receipt_response(
            serde_json::json!({}),
            0,
            ReceiptProofSupport::Supported(snapshot),
            true,
            SelectionLimits::default(),
        )
        .unwrap();
        let ReceiptResponse::WithProof { ssz_proof: Some(proof), .. } = response else {
            panic!("expected proof");
        };
        let size = serde_json::to_vec(&proof).unwrap().len();
        assert!(serde_json::to_vec(&proof.selection).unwrap().len() < size);
        for (response_bytes, expected) in
            [(size, Ok(())), (size - 1, Err(SelectionError::LimitExceeded("response bytes")))]
        {
            assert_eq!(
                verify_whole_receipt_proof(
                    snapshot.block_hash(),
                    0,
                    &proof,
                    snapshot.root(),
                    SelectionLimits { response_bytes, ..SelectionLimits::default() },
                ),
                expected,
            );
        }
    }

    #[test]
    fn receipt_response_rejects_reserved_fields_nonobjects_and_aggregate_limits() {
        for receipt in [serde_json::json!({"sszProof": null}), serde_json::json!([1, 2])] {
            assert_eq!(
                extend_receipt_response(
                    receipt,
                    0,
                    ReceiptProofSupport::Unsupported,
                    true,
                    SelectionLimits::default()
                )
                .unwrap_err(),
                SelectionError::InvalidContext
            );
        }
        let provider = DeterministicProvider::new().unwrap();
        let snapshot =
            provider.lookup(PROGRESSIVE_RECEIPTS_BLOCK_HASH, ObjectKind::Receipts).unwrap();
        for limits in [
            SelectionLimits { selections: 1, targets: 1, ..Default::default() },
            SelectionLimits { witness_hashes: 1, ..Default::default() },
            SelectionLimits { response_bytes: 100, ..Default::default() },
            SelectionLimits { selections: 0, ..Default::default() },
        ] {
            let receipts = (0..snapshot.receipt_snapshot().receipts().len())
                .map(|_| serde_json::json!({}))
                .collect();
            assert!(extend_block_receipt_responses(
                receipts,
                ReceiptProofSupport::Supported(snapshot),
                true,
                limits,
                &AtomicBool::new(false)
            )
            .is_err());
        }
        let receipts = vec![serde_json::json!({"status": "0x1"})];
        let legacy = extend_block_receipt_responses(
            receipts.clone(),
            ReceiptProofSupport::Unsupported,
            false,
            SelectionLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), serde_json::to_value(&receipts).unwrap());
        let unsupported = extend_block_receipt_responses(
            receipts,
            ReceiptProofSupport::Unsupported,
            true,
            SelectionLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(serde_json::to_value(unsupported).unwrap()[0]["sszProof"].is_null());
    }
}
