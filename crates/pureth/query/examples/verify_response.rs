#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_primitives::B256;
use reth_pureth_query::{
    verify_query_response, verify_transaction_receipt, QueryRequest, QueryResponse,
    SelectionLimits, TransactionReceiptRequest, TransactionReceiptResponse,
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
        select_component, ComponentSnapshot, ReceiptSelection, SelectionOperation, SelectionRequest,
    };
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
}
