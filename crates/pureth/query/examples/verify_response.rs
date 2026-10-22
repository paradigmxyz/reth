#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_primitives::B256;
use reth_pureth_query::{
    verify_eip6466_receipt_response, verify_query_response, verify_transaction_receipt,
    QueryRequest, QueryResponse, SelectionLimits, TransactionReceiptRequest,
    TransactionReceiptResponse,
};
use serde::Deserialize;
use std::{
    error::Error,
    io::{self, Read},
};

#[derive(Deserialize)]
#[serde(tag = "method", deny_unknown_fields)]
enum Verification {
    #[serde(rename = "pureth_query")]
    Query { request: QueryRequest, response: QueryResponse, expected_root: B256 },
    #[serde(rename = "pureth_transactionReceipt")]
    Association {
        request: TransactionReceiptRequest,
        response: TransactionReceiptResponse,
        expected_root: B256,
    },
    #[serde(rename = "eth_getTransactionReceipt")]
    Receipt {
        response: serde_json::Value,
        block_hash: B256,
        transaction_index: u64,
        transaction_hash: B256,
        expected_root: B256,
    },
}

fn verify(bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let limits = SelectionLimits::default();
    match serde_json::from_slice(bytes)? {
        Verification::Query { request, response, expected_root } => {
            verify_query_response(&request, &response, expected_root, limits).map_err(|error| {
                io::Error::other(format!("query verification failed: {error:?}"))
            })?;
        }
        Verification::Association { request, response, expected_root } => {
            verify_transaction_receipt(&request, &response, expected_root, limits).map_err(
                |error| io::Error::other(format!("association verification failed: {error:?}")),
            )?;
        }
        Verification::Receipt {
            mut response,
            block_hash,
            transaction_index,
            transaction_hash,
            expected_root,
        } => {
            let proof = response
                .as_object_mut()
                .and_then(|fields| fields.remove("sszProof"))
                .ok_or_else(|| io::Error::other("receipt has no proof"))?;
            let proof = serde_json::from_value(proof)?;
            verify_eip6466_receipt_response(
                &response,
                &proof,
                block_hash,
                transaction_index,
                transaction_hash,
                expected_root,
                limits,
            )
            .map_err(|error| io::Error::other(format!("receipt verification failed: {error:?}")))?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let limits = SelectionLimits::default();
    let maximum = limits.request_bytes + limits.response_bytes + 4096;
    let mut bytes = Vec::new();
    io::stdin().lock().take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(io::Error::other("verification input exceeds size limit").into());
    }
    verify(&bytes)?;
    println!("verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use reth_pureth_query::{
        extend_receipt_response, select_component, ComponentSnapshot, ReceiptProofSupport,
        ReceiptSelection, SelectionOperation, SelectionRequest,
    };
    use reth_pureth_receipt::eip6466::{Eip6466ReceiptSnapshot, Receipts};
    use reth_pureth_transaction::TransactionSnapshot;

    #[test]
    fn saved_query_uses_the_supplied_reference_root_and_rejects_unverified_values() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/transactions.json")).unwrap();
        let bytes: Bytes = fixture["bytes"].as_str().unwrap().parse().unwrap();
        let transactions = TransactionSnapshot::from_ssz(&bytes).unwrap();
        let expected: B256 = fixture["root"].as_str().unwrap().parse().unwrap();
        let mut request = QueryRequest {
            block_hash: B256::repeat_byte(0x11),
            object: "transactions".into(),
            selection: SelectionRequest {
                include_proof: true,
                selections: vec![ReceiptSelection {
                    path: "[0]".into(),
                    operation: SelectionOperation::Whole {},
                }],
            },
        };
        for proved in [true, false] {
            request.selection.include_proof = proved;
            let selected = select_component(
                ComponentSnapshot::Transactions(&transactions),
                &request.selection,
                SelectionLimits::default(),
            )
            .unwrap();
            let response = QueryResponse {
                block_hash: request.block_hash,
                object: request.object.clone(),
                root_context: "reth_experimental_unanchored".into(),
                block_status: "unknown".into(),
                selection: selected.selection,
            };
            let mut record = serde_json::json!({
                "method": "pureth_query",
                "request": request,
                "response": response,
                "expected_root": expected,
            });
            assert_eq!(verify(&serde_json::to_vec(&record).unwrap()).is_ok(), proved);
            record["expected_root"] = serde_json::to_value(B256::ZERO).unwrap();
            assert!(verify(&serde_json::to_vec(&record).unwrap()).is_err());
            record["expected_root"] = serde_json::to_value(expected).unwrap();
            record["unused_context"] = serde_json::json!(true);
            assert!(verify(&serde_json::to_vec(&record).unwrap()).is_err());
        }
    }

    #[test]
    fn saved_receipt_requires_a_proof_and_the_supplied_identity_and_reference_root() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/receipt_selection.json")).unwrap();
        let bytes: Bytes =
            format!("0x{}", fixture["list_bytes"].as_str().unwrap()).parse().unwrap();
        let snapshot =
            Eip6466ReceiptSnapshot::build(Receipts::from_ssz_bytes(&bytes).unwrap()).unwrap();
        let expected: B256 =
            format!("0x{}", fixture["receipt_list_root"].as_str().unwrap()).parse().unwrap();
        assert_eq!(snapshot.root(), expected);
        let block_hash = B256::repeat_byte(0xaa);
        let transaction_hash = B256::repeat_byte(0xbb);
        let receipt = serde_json::json!({
            "type": "0x0", "status": "0x1", "cumulativeGasUsed": "0x5208",
            "gasUsed": "0x5208", "from": alloy_primitives::Address::repeat_byte(0x11),
            "contractAddress": null, "authorities": null,
            "blockHash": block_hash, "blockNumber": "0x1",
            "transactionHash": transaction_hash, "transactionIndex": "0x0",
            "logs": [{
                "address": alloy_primitives::Address::repeat_byte(0x44),
                "topics": [B256::repeat_byte(0x55), B256::repeat_byte(0x66)],
                "data": Bytes::from((0..33).collect::<Vec<u8>>()),
                "blockHash": block_hash, "blockNumber": "0x1",
                "transactionHash": transaction_hash, "transactionIndex": "0x0",
                "logIndex": "0x0", "removed": false
            }]
        });
        let response = extend_receipt_response(
            receipt,
            0,
            ReceiptProofSupport::Retained { block_hash, snapshot: &snapshot },
            true,
            SelectionLimits::default(),
        )
        .unwrap();
        let record = serde_json::json!({
            "method": "eth_getTransactionReceipt", "response": response,
            "block_hash": block_hash, "transaction_index": 0,
            "transaction_hash": transaction_hash, "expected_root": expected,
        });
        verify(&serde_json::to_vec(&record).unwrap()).unwrap();
        for mutation in 0..8 {
            let mut changed = record.clone();
            match mutation {
                0 => changed["expected_root"] = serde_json::json!(B256::ZERO),
                1 => changed["block_hash"] = serde_json::json!(B256::ZERO),
                2 => changed["transaction_hash"] = serde_json::json!(B256::ZERO),
                3 => changed["transaction_index"] = serde_json::json!(1),
                4 => changed["response"]["status"] = serde_json::json!("0x0"),
                5 => changed["response"]["sszProof"] = serde_json::Value::Null,
                6 => {
                    changed["response"].as_object_mut().unwrap().remove("sszProof");
                }
                _ => {
                    changed["response"] = serde_json::Value::Null;
                }
            }
            assert!(verify(&serde_json::to_vec(&changed).unwrap()).is_err(), "mutation {mutation}");
        }
        let mut metadata_only = record;
        metadata_only["response"]["cumulativeGasUsed"] = serde_json::json!("0x1234");
        verify(&serde_json::to_vec(&metadata_only).unwrap()).unwrap();
    }
}
