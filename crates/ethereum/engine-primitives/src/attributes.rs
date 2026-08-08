use alloc::vec::Vec;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{PayloadAttributes, PayloadId};
use core::ops::{Deref, DerefMut};
use reth_payload_primitives::payload_id_with_inclusion_list;

/// Ethereum payload attributes with the EIP-7805 inclusion list used by Bogota Engine API calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthPayloadAttributes {
    /// Payload attributes shared with earlier Engine API versions.
    #[serde(flatten)]
    pub inner: PayloadAttributes,
    /// Transactions that the payload builder must attempt to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusion_list_transactions: Option<Vec<Bytes>>,
}

impl EthPayloadAttributes {
    /// Wraps pre-Bogota payload attributes.
    pub const fn new(inner: PayloadAttributes) -> Self {
        Self { inner, inclusion_list_transactions: None }
    }

    /// Sets the EIP-7805 inclusion list.
    pub fn with_inclusion_list(mut self, transactions: Vec<Bytes>) -> Self {
        self.inclusion_list_transactions = Some(transactions);
        self
    }
}

impl Deref for EthPayloadAttributes {
    type Target = PayloadAttributes;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for EthPayloadAttributes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<PayloadAttributes> for EthPayloadAttributes {
    fn from(value: PayloadAttributes) -> Self {
        Self::new(value)
    }
}

impl reth_payload_primitives::PayloadAttributes for EthPayloadAttributes {
    fn payload_id(&self, parent_hash: &B256) -> PayloadId {
        payload_id_with_inclusion_list(
            parent_hash,
            &self.inner,
            self.inclusion_list_transactions.as_deref(),
        )
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.inner.withdrawals()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root()
    }

    fn slot_number(&self) -> Option<u64> {
        self.inner.slot_number()
    }

    fn target_gas_limit(&self) -> Option<u64> {
        self.inner.target_gas_limit()
    }

    fn inclusion_list_transactions(&self) -> Option<&[Bytes]> {
        self.inclusion_list_transactions.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_payload_primitives::{
        validate_inclusion_list_presence, EngineApiMessageVersion, EngineObjectValidationError,
        PayloadAttributes as _, VersionSpecificValidationError,
    };

    #[test]
    fn inclusion_list_changes_payload_id() {
        let parent = B256::repeat_byte(0x11);
        let attributes = EthPayloadAttributes::default();
        let with_empty_list = attributes.clone().with_inclusion_list(Vec::new());
        let with_list = attributes.clone().with_inclusion_list(vec![Bytes::from_static(&[0x01])]);

        assert_ne!(attributes.payload_id(&parent), with_empty_list.payload_id(&parent));
        assert_ne!(attributes.payload_id(&parent), with_list.payload_id(&parent));
    }

    #[test]
    fn inclusion_list_is_required_only_for_forkchoice_v5() {
        let without_list = EthPayloadAttributes::default();
        assert!(matches!(
            validate_inclusion_list_presence(EngineApiMessageVersion::V5, &without_list),
            Err(EngineObjectValidationError::PayloadAttributes(
                VersionSpecificValidationError::NoInclusionList
            ))
        ));

        let with_list = without_list.with_inclusion_list(Vec::new());
        assert!(validate_inclusion_list_presence(EngineApiMessageVersion::V5, &with_list).is_ok());
        assert!(matches!(
            validate_inclusion_list_presence(EngineApiMessageVersion::V4, &with_list),
            Err(EngineObjectValidationError::PayloadAttributes(
                VersionSpecificValidationError::InclusionListNotSupported
            ))
        ));
    }

    #[test]
    fn inclusion_list_serde_distinguishes_empty_from_missing() {
        let attributes = EthPayloadAttributes::default().with_inclusion_list(Vec::new());
        let value = serde_json::to_value(&attributes).unwrap();
        assert_eq!(value["inclusionListTransactions"], serde_json::json!([]));

        let decoded: EthPayloadAttributes = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.inclusion_list_transactions, Some(Vec::new()));
    }
}
