//! Scroll specific types related to transactions.

use alloy_consensus::{Transaction as _, Typed2718};
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Address, BlockHash, Bytes, ChainId, TxKind, B256, U256};
use alloy_serde::OtherFields;
use scroll_alloy_consensus::ScrollTxEnvelope;
use serde::{Deserialize, Serialize};

mod request;
pub use request::ScrollTransactionRequest;

/// Scroll Transaction type
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, derive_more::Deref, derive_more::DerefMut,
)]
#[serde(try_from = "tx_serde::TransactionSerdeHelper", into = "tx_serde::TransactionSerdeHelper")]
#[cfg_attr(all(any(test, feature = "arbitrary"), feature = "k256"), derive(arbitrary::Arbitrary))]
pub struct Transaction {
    /// Ethereum Transaction Types
    #[deref]
    #[deref_mut]
    pub inner: alloy_rpc_types_eth::Transaction<ScrollTxEnvelope>,
}

impl Typed2718 for Transaction {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl alloy_consensus::Transaction for Transaction {
    fn chain_id(&self) -> Option<ChainId> {
        self.inner.chain_id()
    }

    fn nonce(&self) -> u64 {
        self.inner.nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.inner.gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.inner.max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.inner.max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.inner.max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.inner.priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.inner.effective_gas_price(base_fee)
    }

    fn is_dynamic_fee(&self) -> bool {
        self.inner.is_dynamic_fee()
    }

    fn kind(&self) -> TxKind {
        self.inner.kind()
    }

    fn is_create(&self) -> bool {
        self.inner.is_create()
    }

    fn to(&self) -> Option<Address> {
        self.inner.to()
    }

    fn value(&self) -> U256 {
        self.inner.value()
    }

    fn input(&self) -> &Bytes {
        self.inner.input()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.inner.access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.inner.blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.inner.authorization_list()
    }
}

impl alloy_network_primitives::TransactionResponse for Transaction {
    fn tx_hash(&self) -> alloy_primitives::TxHash {
        self.inner.tx_hash()
    }

    fn block_hash(&self) -> Option<BlockHash> {
        self.inner.block_hash()
    }

    fn block_number(&self) -> Option<u64> {
        self.inner.block_number()
    }

    fn transaction_index(&self) -> Option<u64> {
        self.inner.transaction_index()
    }

    fn from(&self) -> Address {
        self.inner.from()
    }
}

/// Scroll specific transaction fields
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollL1MessageTransactionFields {
    /// The index of the transaction in the message queue.
    pub queue_index: u64,
    /// The sender of the transaction on the L1.
    pub sender: Address,
}

impl From<ScrollL1MessageTransactionFields> for OtherFields {
    fn from(value: ScrollL1MessageTransactionFields) -> Self {
        serde_json::to_value(value).unwrap().try_into().unwrap()
    }
}

impl AsRef<ScrollTxEnvelope> for Transaction {
    fn as_ref(&self) -> &ScrollTxEnvelope {
        self.inner.as_ref()
    }
}

mod tx_serde {
    //! Helper module for serializing and deserializing Scroll [`Transaction`].
    //!
    //! This is needed because we might need to deserialize the `from` field into both
    //! [`alloy_rpc_types_eth::Transaction::from`] and
    //! [`scroll_alloy_consensus::TxL1Message`].
    //!
    //! Additionally, we need similar logic for the `gasPrice` field

    use super::*;
    use alloy_consensus::transaction::Recovered;
    use serde::de::Error;

    /// Helper struct which will be flattened into the transaction and will only contain `from`
    /// field if inner [`ScrollTxEnvelope`] did not consume it.
    #[derive(Serialize, Deserialize)]
    struct OptionalFields {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Address>,
        #[serde(
            default,
            rename = "gasPrice",
            skip_serializing_if = "Option::is_none",
            with = "alloy_serde::quantity::opt"
        )]
        effective_gas_price: Option<u128>,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct TransactionSerdeHelper {
        #[serde(flatten)]
        inner: ScrollTxEnvelope,
        #[serde(default)]
        block_hash: Option<BlockHash>,
        #[serde(default, with = "alloy_serde::quantity::opt")]
        block_number: Option<u64>,
        #[serde(default, with = "alloy_serde::quantity::opt")]
        transaction_index: Option<u64>,
        #[serde(flatten)]
        other: OptionalFields,
    }

    impl From<Transaction> for TransactionSerdeHelper {
        fn from(value: Transaction) -> Self {
            let Transaction {
                inner:
                    alloy_rpc_types_eth::Transaction {
                        inner: recovered,
                        block_hash,
                        block_number,
                        transaction_index,
                        effective_gas_price,
                    },
                ..
            } = value;

            // if inner transaction has its own `gasPrice` don't serialize it in this struct.
            let effective_gas_price =
                effective_gas_price.filter(|_| recovered.gas_price().is_none());
            let (inner, from) = recovered.into_parts();

            Self {
                inner,
                block_hash,
                block_number,
                transaction_index,
                other: OptionalFields { from: Some(from), effective_gas_price },
            }
        }
    }

    impl TryFrom<TransactionSerdeHelper> for Transaction {
        type Error = serde_json::Error;

        fn try_from(value: TransactionSerdeHelper) -> Result<Self, Self::Error> {
            let TransactionSerdeHelper {
                inner,
                block_hash,
                block_number,
                transaction_index,
                other,
            } = value;

            // Try to get `from` field from inner envelope or from `MaybeFrom`, otherwise return
            // error
            let from = if let Some(from) = other.from {
                from
            } else if let ScrollTxEnvelope::L1Message(tx) = &inner {
                tx.sender
            } else {
                return Err(serde_json::Error::custom("missing `from` field"));
            };

            let effective_gas_price = other.effective_gas_price.or_else(|| inner.gas_price());
            let recovered = Recovered::new_unchecked(inner, from);

            Ok(Self {
                inner: alloy_rpc_types_eth::Transaction {
                    inner: recovered,
                    block_hash,
                    block_number,
                    transaction_index,
                    effective_gas_price,
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn can_deserialize_deposit() {
        // cast rpc eth_getTransactionByHash
        // 0x5c1c3785c8bf5d7f1cb714abd1d22e32642887215602c3a14a5e9ee105bad6aa --rpc-url https://rpc.scroll.io
        let rpc_tx = r#"{"blockHash":"0x018ed80ea8340984a1f4841490284d6e51d71f9e9411feeca41e007a89fbfdff","blockNumber":"0xb81121","from":"0x7885bcbd5cecef1336b5300fb5186a12ddd8c478","gas":"0x1e8480","gasPrice":"0x0","hash":"0x5c1c3785c8bf5d7f1cb714abd1d22e32642887215602c3a14a5e9ee105bad6aa","input":"0x8ef1332e000000000000000000000000c186fa914353c44b2e33ebe05f21846f1048beda0000000000000000000000003bad7ad0728f9917d1bf08af5782dcbd516cdd96000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e7ba000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044493a4f846ffc1507cbfe98a2b0ba1f06ea7e4eb749c001f78f6cb5540daa556a0566322a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","to":"0x781e90f1c8fc4611c9b7497c3b47f99ef6969cbc","transactionIndex":"0x0","value":"0x0","type":"0x7e","v":"0x0","r":"0x0","s":"0x0","sender":"0x7885bcbd5cecef1336b5300fb5186a12ddd8c478","queueIndex":"0xe7ba0", "yParity":"0x0"}"#;

        let tx = serde_json::from_str::<Transaction>(rpc_tx).unwrap();

        let ScrollTxEnvelope::L1Message(inner) = tx.as_ref() else {
            panic!("Expected deposit transaction");
        };
        assert_eq!(inner.sender, address!("7885bcbd5cecef1336b5300fb5186a12ddd8c478"));
        assert_eq!(inner.queue_index, 0xe7ba0);
        assert_eq!(tx.inner.effective_gas_price, Some(0));

        let deserialized = serde_json::to_value(&tx).unwrap();
        let expected = serde_json::from_str::<serde_json::Value>(rpc_tx).unwrap();
        similar_asserts::assert_eq!(deserialized, expected);
    }
}
