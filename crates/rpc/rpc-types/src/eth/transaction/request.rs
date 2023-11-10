use crate::eth::transaction::{
    typed::{
        BlobTransactionSidecar, EIP1559TransactionRequest, EIP2930TransactionRequest,
        LegacyTransactionRequest, TransactionKind, TypedTransactionRequest,
    },
    AccessList,
};
use alloy_primitives::{Address, Bytes, B256, U128, U256, U64, U8};
use serde::{Deserialize, Serialize};

/// Represents _all_ transaction requests received from RPC
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct TransactionRequest {
    /// from address
    pub from: Option<Address>,
    /// to address
    pub to: Option<Address>,
    /// legacy, gas Price
    pub gas_price: Option<U128>,
    /// max base fee per gas sender is willing to pay
    pub max_fee_per_gas: Option<U128>,
    /// miner tip
    pub max_priority_fee_per_gas: Option<U128>,
    /// gas
    pub gas: Option<U256>,
    /// value of th tx in wei
    pub value: Option<U256>,
    /// Any additional data sent
    #[serde(alias = "data")]
    pub input: Option<Bytes>,
    /// Transaction nonce
    pub nonce: Option<U64>,
    /// warm storage access pre-payment
    pub access_list: Option<AccessList>,
    /// EIP-2718 type
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transaction_type: Option<U8>,
    /// max fee per blob gas for EIP-4844 transactions
    pub max_fee_per_blob_gas: Option<U128>,
    /// blob versioned hashes for EIP-4844 transactions.
    pub blob_versioned_hashes: Option<Vec<B256>>,
    /// sidecar for EIP-4844 transactions
    pub sidecar: Option<BlobTransactionSidecar>,
}

// == impl TransactionRequest ==

impl TransactionRequest {
    /// Converts the request into a [`TypedTransactionRequest`]
    ///
    /// Returns None if mutual exclusive fields `gasPrice` and `max_fee_per_gas` are either missing
    /// or both set.
    pub fn into_typed_request(self) -> Option<TypedTransactionRequest> {
        let TransactionRequest {
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input: data,
            nonce,
            mut access_list,
            max_fee_per_blob_gas,
            blob_versioned_hashes,
            sidecar,
            ..
        } = self;
        match (
            gas_price,
            max_fee_per_gas,
            access_list.take(),
            max_fee_per_blob_gas,
            blob_versioned_hashes,
            sidecar,
        ) {
            // legacy transaction
            // gas price required
            (Some(_), None, None, None, None, None) => {
                Some(TypedTransactionRequest::Legacy(LegacyTransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: gas_price.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.unwrap_or_default(),
                    kind: match to {
                        Some(to) => TransactionKind::Call(to),
                        None => TransactionKind::Create,
                    },
                    chain_id: None,
                }))
            }
            // EIP2930
            // if only accesslist is set, and no eip1599 fees
            (_, None, Some(access_list), None, None, None) => {
                Some(TypedTransactionRequest::EIP2930(EIP2930TransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: gas_price.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.unwrap_or_default(),
                    kind: match to {
                        Some(to) => TransactionKind::Call(to),
                        None => TransactionKind::Create,
                    },
                    chain_id: 0,
                    access_list,
                }))
            }
            // EIP1559
            // if 4844 fields missing
            // gas_price, max_fee_per_gas, access_list, max_fee_per_blob_gas, blob_versioned_hashes,
            // sidecar,
            (None, _, _, None, None, None) => {
                // Empty fields fall back to the canonical transaction schema.
                Some(TypedTransactionRequest::EIP1559(EIP1559TransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    max_fee_per_gas: max_fee_per_gas.unwrap_or_default(),
                    max_priority_fee_per_gas: max_priority_fee_per_gas.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.unwrap_or_default(),
                    kind: match to {
                        Some(to) => TransactionKind::Call(to),
                        None => TransactionKind::Create,
                    },
                    chain_id: 0,
                    access_list: access_list.unwrap_or_default(),
                }))
            }
            // EIP4884
            // all blob fields required
            (
                None,
                _,
                _,
                Some(max_fee_per_blob_gas),
                Some(blob_versioned_hashes),
                Some(sidecar),
            ) => {
                // As per the EIP, we follow the same semantics as EIP-1559.
                Some(TypedTransactionRequest::EIP4844(crate::EIP4844TransactionRequest {
                    chain_id: 0,
                    nonce: nonce.unwrap_or_default(),
                    max_priority_fee_per_gas: max_priority_fee_per_gas.unwrap_or_default(),
                    max_fee_per_gas: max_fee_per_gas.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.unwrap_or_default(),
                    kind: match to {
                        Some(to) => TransactionKind::Call(to),
                        None => TransactionKind::Create,
                    },
                    access_list: access_list.unwrap_or_default(),

                    // eip-4844 specific.
                    max_fee_per_blob_gas,
                    blob_versioned_hashes,
                    sidecar,
                }))
            }

            _ => None,
        }
    }

    /// Sets the gas limit for the transaction.
    pub fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas = Some(U256::from(gas_limit));
        self
    }

    /// Sets the nonce for the transaction.
    pub fn nonce(mut self, nonce: u64) -> Self {
        self.nonce = Some(U64::from(nonce));
        self
    }

    /// Sets the maximum fee per gas for the transaction.
    pub fn max_fee_per_gas(mut self, max_fee_per_gas: u128) -> Self {
        self.max_fee_per_gas = Some(U128::from(max_fee_per_gas));
        self
    }

    /// Sets the maximum priority fee per gas for the transaction.
    pub fn max_priority_fee_per_gas(mut self, max_priority_fee_per_gas: u128) -> Self {
        self.max_priority_fee_per_gas = Some(U128::from(max_priority_fee_per_gas));
        self
    }

    /// Sets the recipient address for the transaction.
    pub fn to(mut self, to: Address) -> Self {
        self.to = Some(to);
        self
    }
    /// Sets the value (amount) for the transaction.

    pub fn value(mut self, value: u128) -> Self {
        self.value = Some(U256::from(value));
        self
    }

    /// Sets the access list for the transaction.
    pub fn access_list(mut self, access_list: AccessList) -> Self {
        self.access_list = Some(access_list);
        self
    }

    /// Sets the input data for the transaction.
    pub fn input(mut self, input: impl Into<Bytes>) -> Self {
        self.input = Some(input.into());
        self
    }

    /// Sets the transactions type for the transactions.
    pub fn transaction_type(mut self, transaction_type: u8) -> Self {
        self.transaction_type = Some(U8::from(transaction_type));
        self
    }
}
