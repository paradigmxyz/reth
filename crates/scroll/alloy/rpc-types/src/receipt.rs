//! Receipt types for RPC

use alloy_consensus::{Receipt, ReceiptWithBloom};
use alloy_serde::OtherFields;
use serde::{Deserialize, Serialize};

use scroll_alloy_consensus::ScrollReceiptEnvelope;

/// Scroll Transaction Receipt type
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[doc(alias = "ScrollTxReceipt")]
pub struct ScrollTransactionReceipt {
    /// Regular eth transaction receipt including deposit receipts
    #[serde(flatten)]
    pub inner:
        alloy_rpc_types_eth::TransactionReceipt<ScrollReceiptEnvelope<alloy_rpc_types_eth::Log>>,
    /// L1 fee for the transaction.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_fee: Option<u128>,
}

impl alloy_network_primitives::ReceiptResponse for ScrollTransactionReceipt {
    fn contract_address(&self) -> Option<alloy_primitives::Address> {
        self.inner.contract_address
    }

    fn status(&self) -> bool {
        self.inner.inner.status()
    }

    fn block_hash(&self) -> Option<alloy_primitives::BlockHash> {
        self.inner.block_hash
    }

    fn block_number(&self) -> Option<u64> {
        self.inner.block_number
    }

    fn transaction_hash(&self) -> alloy_primitives::TxHash {
        self.inner.transaction_hash
    }

    fn transaction_index(&self) -> Option<u64> {
        self.inner.transaction_index()
    }

    fn gas_used(&self) -> u64 {
        self.inner.gas_used()
    }

    fn effective_gas_price(&self) -> u128 {
        self.inner.effective_gas_price()
    }

    fn blob_gas_used(&self) -> Option<u64> {
        self.inner.blob_gas_used()
    }

    fn blob_gas_price(&self) -> Option<u128> {
        self.inner.blob_gas_price()
    }

    fn from(&self) -> alloy_primitives::Address {
        self.inner.from()
    }

    fn to(&self) -> Option<alloy_primitives::Address> {
        self.inner.to()
    }

    fn cumulative_gas_used(&self) -> u64 {
        self.inner.cumulative_gas_used()
    }

    fn state_root(&self) -> Option<alloy_primitives::B256> {
        self.inner.state_root()
    }
}

/// Additional fields for Scroll transaction receipts: <https://github.com/scroll-tech/go-ethereum/blob/develop/core/types/receipt.go#L78>
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[doc(alias = "ScrollTxReceiptFields")]
pub struct ScrollTransactionReceiptFields {
    /// L1 fee for the transaction.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_fee: Option<u128>,
}

impl From<ScrollTransactionReceiptFields> for OtherFields {
    fn from(value: ScrollTransactionReceiptFields) -> Self {
        serde_json::to_value(value).unwrap().try_into().unwrap()
    }
}

impl From<ScrollTransactionReceipt> for ScrollReceiptEnvelope<alloy_primitives::Log> {
    fn from(value: ScrollTransactionReceipt) -> Self {
        let inner_envelope = value.inner.inner;

        /// Helper function to convert the inner logs within a [`ReceiptWithBloom`] from RPC to
        /// consensus types.
        #[inline(always)]
        fn convert_standard_receipt(
            receipt: ReceiptWithBloom<Receipt<alloy_rpc_types_eth::Log>>,
        ) -> ReceiptWithBloom<Receipt<alloy_primitives::Log>> {
            let ReceiptWithBloom { logs_bloom, receipt } = receipt;

            let consensus_logs = receipt.logs.into_iter().map(|log| log.inner).collect();
            ReceiptWithBloom {
                receipt: Receipt {
                    status: receipt.status,
                    cumulative_gas_used: receipt.cumulative_gas_used,
                    logs: consensus_logs,
                },
                logs_bloom,
            }
        }

        match inner_envelope {
            ScrollReceiptEnvelope::Legacy(receipt) => {
                Self::Legacy(convert_standard_receipt(receipt))
            }
            ScrollReceiptEnvelope::Eip2930(receipt) => {
                Self::Eip2930(convert_standard_receipt(receipt))
            }
            ScrollReceiptEnvelope::Eip1559(receipt) => {
                Self::Eip1559(convert_standard_receipt(receipt))
            }
            ScrollReceiptEnvelope::L1Message(receipt) => {
                Self::L1Message(convert_standard_receipt(receipt))
            }
            _ => unreachable!("Unsupported ScrollReceiptEnvelope variant"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // <https://github.com/alloy-rs/op-alloy/issues/18>
    #[test]
    fn parse_rpc_receipt() {
        let s = r#"{
        "blockHash": "0x9e6a0fb7e22159d943d760608cc36a0fb596d1ab3c997146f5b7c55c8c718c67",
        "blockNumber": "0x6cfef89",
        "contractAddress": null,
        "cumulativeGasUsed": "0xfa0d",
        "effectiveGasPrice": "0x0",
        "from": "0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001",
        "gasUsed": "0xfa0d",
        "logs": [],
        "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "status": "0x1",
        "to": "0x4200000000000000000000000000000000000015",
        "transactionHash": "0xb7c74afdeb7c89fb9de2c312f49b38cb7a850ba36e064734c5223a477e83fdc9",
        "transactionIndex": "0x0",
        "type": "0x7e"
    }"#;

        let receipt: ScrollTransactionReceipt = serde_json::from_str(s).unwrap();
        let value = serde_json::to_value(&receipt).unwrap();
        let expected_value = serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(value, expected_value);
    }

    #[test]
    fn serialize_empty_scroll_transaction_receipt_fields_struct() {
        let scroll_fields = ScrollTransactionReceiptFields::default();

        let json = serde_json::to_value(scroll_fields).unwrap();
        assert_eq!(json, json!({}));
    }
}
