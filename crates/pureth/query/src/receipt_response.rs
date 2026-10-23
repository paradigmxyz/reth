use crate::{
    selection::{encoded_size, remaining_materialization, select_with_source, source_budget},
    verify_receipt_selection, ReceiptSelection, SelectionError, SelectionLimits,
    SelectionOperation, SelectionProofFormat, SelectionRequest, SelectionResponse,
};
use alloy_primitives::B256;
use reth_pureth_receipt::{Eip6466ReceiptSnapshot, ProviderSnapshot, RootContext};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{
    convert_eip6466_block_receipts, convert_eip6466_block_receipts_from_retained,
    convert_eip6466_receipt, convert_eip6466_receipt_from_retained,
    verify_eip6466_receipt_response,
};
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{
    ExecutionHeaderConverter, ExecutionReceiptConverter, ExecutionReceiptEnvelope,
    ExecutionRpcReceipt, ExecutionRpcTypes,
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
    Retained { block_hash: B256, snapshot: &'a Eip6466ReceiptSnapshot },
    Unsupported,
}

impl<'a> ReceiptProofSupport<'a> {
    fn source(self) -> Result<Option<(B256, &'a Eip6466ReceiptSnapshot)>, SelectionError> {
        match self {
            Self::Unsupported => Ok(None),
            Self::Retained { block_hash, snapshot } => Ok(Some((block_hash, snapshot))),
            Self::Supported(snapshot) => {
                if snapshot.root_context() != RootContext::RethExperimentalUnanchored {
                    return Err(SelectionError::InvalidContext);
                }
                Ok(Some((snapshot.block_hash(), snapshot.receipt_snapshot())))
            }
        }
    }
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

fn proof(block_hash: B256, index: u64, selection: SelectionResponse) -> WholeReceiptProof {
    WholeReceiptProof {
        block_hash,
        receipt_index: index,
        root_context: "reth_experimental_unanchored".into(),
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
    let receipt_bytes = encoded_size(&receipt, limits.response_bytes, "response bytes")?;
    check_receipt(&receipt)?;
    let ssz_proof = match support.source()? {
        None => None,
        Some((block_hash, snapshot)) => {
            let cancelled = AtomicBool::new(false);
            let mut source = source_budget(snapshot, limits, &cancelled)?;
            crate::selection::add(
                &mut source,
                receipt_bytes
                    .checked_mul(2)
                    .ok_or(SelectionError::LimitExceeded("source bytes"))?,
                limits.source_bytes,
                "source bytes",
            )?;
            let selection = select_with_source(
                snapshot,
                &request([index]),
                limits,
                &cancelled,
                SelectionProofFormat::SingleBranch,
                source,
            )?;
            Some(proof(block_hash, index, selection))
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
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    if !enabled {
        return Ok(receipts.into_iter().map(ReceiptResponse::Legacy).collect());
    }
    if support.source()?.is_some_and(|(_, retained)| retained.receipts().len() != receipts.len()) {
        return Err(SelectionError::InvalidContext);
    }
    let indices = (0..receipts.len() as u64).collect::<Vec<_>>();
    extend_indexed_receipt_responses(receipts, &indices, support, enabled, limits, cancelled)
}

pub(super) fn extend_indexed_receipt_responses<T: Serialize>(
    receipts: Vec<T>,
    indices: &[u64],
    support: ReceiptProofSupport<'_>,
    enabled: bool,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<Vec<ReceiptResponse<T>>, SelectionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled);
    }
    if indices.len() != receipts.len() {
        return Err(SelectionError::InvalidContext);
    }
    if !enabled {
        return Ok(receipts.into_iter().map(ReceiptResponse::Legacy).collect());
    }
    let receipt_bytes = encoded_size(&receipts, limits.response_bytes, "response bytes")?;
    for receipt in &receipts {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        check_receipt(receipt)?;
    }
    let mut proofs = Vec::new();
    if let Some((block_hash, retained)) = support.source()? {
        let mut source = source_budget(retained, limits, cancelled)?;
        crate::selection::add(
            &mut source,
            receipt_bytes.checked_mul(2).ok_or(SelectionError::LimitExceeded("source bytes"))?,
            limits.source_bytes,
            "source bytes",
        )?;
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
                &request(indices[start..end].iter().copied()),
                limits,
                cancelled,
                SelectionProofFormat::SingleBranch,
                source,
            )?;
            for (&index, result) in indices[start..end].iter().zip(selection.results) {
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
                    block_hash,
                    index,
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
        proof.root_context != "reth_experimental_unanchored"
    {
        return Err(SelectionError::InvalidContext);
    }
    verify_receipt_selection(&request([index]), &proof.selection, expected_root, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;

    #[test]
    fn single_receipt_proof_reserves_existing_response_work() {
        let snapshot = test_utils::snapshot(1);
        let mut limits = SelectionLimits::default();
        let source =
            source_budget(snapshot.receipt_snapshot(), limits, &AtomicBool::new(false)).unwrap();
        let receipt = serde_json::json!({"payload": "x".repeat(128 * 1024)});
        let receipt_bytes =
            encoded_size(&receipt, limits.response_bytes, "response bytes").unwrap();
        limits.source_bytes = source + receipt_bytes - 1;
        assert!(crate::select_receipt_snapshot(snapshot.receipt_snapshot(), &request([0]), limits)
            .is_ok());
        assert!(matches!(
            extend_receipt_response(
                receipt,
                0,
                ReceiptProofSupport::Supported(&snapshot),
                true,
                limits
            ),
            Err(SelectionError::LimitExceeded("source bytes"))
        ));
    }

    #[test]
    fn indexed_receipt_proofs_preserve_nonzero_indices_order_and_duplicates() {
        let snapshot = test_utils::snapshot(2);
        let support = ReceiptProofSupport::Supported(&snapshot);
        let receipts = vec![serde_json::json!({"transactionIndex": "0x1"}); 3];
        let indices = [1, 0, 1];
        let responses = extend_indexed_receipt_responses(
            receipts.clone(),
            &indices,
            support,
            true,
            SelectionLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(responses.len(), indices.len());
        for (response, index) in responses.iter().zip(indices) {
            let ReceiptResponse::WithProof { ssz_proof: Some(proof), .. } = response else {
                panic!("expected proof")
            };
            verify_whole_receipt_proof(
                snapshot.block_hash(),
                index,
                proof,
                snapshot.root(),
                SelectionLimits::default(),
            )
            .unwrap();
        }
        assert!(matches!(
            extend_block_receipt_responses(
                receipts.clone(),
                support,
                true,
                SelectionLimits::default(),
                &AtomicBool::new(false)
            ),
            Err(SelectionError::InvalidContext)
        ));
        assert!(matches!(
            extend_indexed_receipt_responses(
                receipts.clone(),
                &[1],
                support,
                true,
                SelectionLimits::default(),
                &AtomicBool::new(false)
            ),
            Err(SelectionError::InvalidContext)
        ));
        assert!(matches!(
            extend_block_receipt_responses(
                receipts,
                support,
                false,
                SelectionLimits::default(),
                &AtomicBool::new(true)
            ),
            Err(SelectionError::Cancelled)
        ));
    }

    #[test]
    fn retained_support_preserves_proof_policy_output_and_limits() {
        let snapshot = test_utils::snapshot(2);
        let old = ReceiptProofSupport::Supported(&snapshot);
        let retained = ReceiptProofSupport::Retained {
            block_hash: snapshot.block_hash(),
            snapshot: snapshot.receipt_snapshot(),
        };
        let receipt = serde_json::json!({ "transactionIndex": "0x0" });
        for enabled in [false, true] {
            let response = |support| {
                serde_json::to_value(
                    extend_receipt_response(
                        receipt.clone(),
                        0,
                        support,
                        enabled,
                        SelectionLimits::default(),
                    )
                    .unwrap(),
                )
                .unwrap()
            };
            assert_eq!(response(old), response(retained));
        }
        let receipts = vec![receipt.clone(), receipt];
        let batch = |support| {
            serde_json::to_value(
                extend_block_receipt_responses(
                    receipts.clone(),
                    support,
                    true,
                    SelectionLimits::default(),
                    &AtomicBool::new(false),
                )
                .unwrap(),
            )
            .unwrap()
        };
        assert_eq!(batch(old), batch(retained));
        for support in [old, retained] {
            assert_eq!(
                extend_receipt_response(
                    receipts[0].clone(),
                    2,
                    support,
                    true,
                    SelectionLimits::default(),
                )
                .unwrap_err(),
                SelectionError::OutOfBounds
            );
            assert_eq!(
                extend_block_receipt_responses(
                    receipts.clone(),
                    support,
                    true,
                    SelectionLimits { witness_hashes: 0, ..Default::default() },
                    &AtomicBool::new(false),
                )
                .unwrap_err(),
                SelectionError::LimitExceeded("witness hashes")
            );
            assert_eq!(
                extend_block_receipt_responses(
                    receipts.clone(),
                    support,
                    true,
                    SelectionLimits::default(),
                    &AtomicBool::new(true),
                )
                .unwrap_err(),
                SelectionError::Cancelled
            );
        }
    }
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
        let snapshot = test_utils::snapshot(1);
        let supported = extend_receipt_response(
            receipt.clone(),
            0,
            ReceiptProofSupport::Supported(&snapshot),
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
        let mut obsolete = proof;
        obsolete.root_context = "deterministic_test_data".into();
        assert_eq!(
            verify_whole_receipt_proof(
                snapshot.block_hash(),
                0,
                &obsolete,
                snapshot.root(),
                SelectionLimits::default()
            ),
            Err(SelectionError::InvalidContext)
        );
        assert!(extend_receipt_response(
            receipt,
            1,
            ReceiptProofSupport::Supported(&snapshot),
            true,
            SelectionLimits::default()
        )
        .is_err());
    }
    #[test]
    fn block_receipt_proofs_share_the_snapshot_but_verify_each_complete_element() {
        let snapshot = test_utils::snapshot(6);
        let receipts = (0..snapshot.receipt_snapshot().receipts().len())
            .map(|index| serde_json::json!({"transactionIndex": index}))
            .collect();
        let responses = extend_block_receipt_responses(
            receipts,
            ReceiptProofSupport::Supported(&snapshot),
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
            ReceiptProofSupport::Supported(&snapshot),
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
            ReceiptProofSupport::Supported(&snapshot),
            true,
            SelectionLimits::default(),
            &AtomicBool::new(true)
        )
        .is_err());
    }

    #[test]
    fn whole_receipt_verification_caps_the_complete_proof_envelope() {
        let snapshot = test_utils::snapshot(1);
        let response = extend_receipt_response(
            serde_json::json!({}),
            0,
            ReceiptProofSupport::Supported(&snapshot),
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
        let snapshot = test_utils::snapshot(6);
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
                ReceiptProofSupport::Supported(&snapshot),
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
