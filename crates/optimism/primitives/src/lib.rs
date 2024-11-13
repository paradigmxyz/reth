//! Standalone crate for Optimism-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod bedrock;
pub mod op_tx_type;

use alloy_primitives::{bytes, Bytes, TxKind, Uint, B256};

use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_rlp::{Decodable, Encodable, Result};
use op_tx_type::OpTxType;
use reth_primitives::revm_primitives::{AccessList, SignedAuthorization};
use reth_primitives_traits::{InMemorySize, TransactionExt};

use op_alloy_consensus::{OpTxEnvelope, OpTypedTransaction};

#[cfg(feature = "arbitrary")]
use arbitrary::Arbitrary;

#[cfg_attr(feature = "arbitrary", derive(Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Optimistic transaction type.
pub struct OpTransaction(OpTypedTransaction);

impl Default for OpTransaction {
    fn default() -> Self {
        Self(OpTypedTransaction::Legacy(TxLegacy::default()))
    }
}

impl Encodable for OpTransaction {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match &self.0 {
            OpTypedTransaction::Legacy(tx) => tx.encode(out),
            OpTypedTransaction::Eip2930(tx) => tx.encode(out),
            OpTypedTransaction::Eip1559(tx) => tx.encode(out),
            OpTypedTransaction::Eip7702(tx) => tx.encode(out),
            OpTypedTransaction::Deposit(tx) => tx.encode(out),
        }
    }
}

impl Decodable for OpTransaction {
    fn decode(data: &mut &[u8]) -> Result<Self> {
        let tx_envelope = OpTxEnvelope::decode(data)?;
        Ok(Self(tx_envelope.into()))
    }
}

impl reth_codecs::Compact for OpTransaction {
    fn to_compact<B>(&self, out: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match &self.0 {
            OpTypedTransaction::Legacy(tx) => tx.to_compact(out),
            OpTypedTransaction::Eip2930(tx) => tx.to_compact(out),
            OpTypedTransaction::Eip1559(tx) => tx.to_compact(out),
            OpTypedTransaction::Eip7702(tx) => tx.to_compact(out),
            OpTypedTransaction::Deposit(tx) => tx.to_compact(out),
        }
    }

    fn from_compact(_buf: &[u8], _len: usize) -> (Self, &[u8]) {
        todo!()
    }
}

#[cfg(any(feature = "test-utils", feature = "arbitrary"))]
impl MaybeArbitrary for OpTransaction {}

#[cfg(any(feature = "test-utils", feature = "arbitrary"))]
impl MaybeArbitrary for OpTransaction {}

impl alloy_consensus::Transaction for OpTransaction {
    fn chain_id(&self) -> Option<u64> {
        self.0.chain_id()
    }

    fn nonce(&self) -> u64 {
        self.0.nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.0.gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.0.gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.0.max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.0.max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.0.max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.0.priority_fee_or_price()
    }

    fn kind(&self) -> TxKind {
        self.0.kind()
    }

    fn value(&self) -> Uint<256, 4> {
        self.0.value()
    }

    fn input(&self) -> &Bytes {
        self.0.input()
    }

    fn ty(&self) -> u8 {
        self.0.ty()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.0.access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.0.blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.0.authorization_list()
    }

    fn is_dynamic_fee(&self) -> bool {
        self.0.is_dynamic_fee()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.0.effective_gas_price(base_fee)
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        self.0.effective_tip_per_gas(base_fee)
    }
}

impl TransactionExt for OpTransaction {
    type Type = OpTxType;

    fn signature_hash(&self) -> B256 {
        match &self.0 {
            OpTypedTransaction::Legacy(tx) => tx.signature_hash(),
            OpTypedTransaction::Eip2930(tx) => tx.signature_hash(),
            OpTypedTransaction::Eip1559(tx) => tx.signature_hash(),
            OpTypedTransaction::Eip7702(tx) => tx.signature_hash(),
            OpTypedTransaction::Deposit(tx) => tx.tx_hash(),
        }
    }
}

impl InMemorySize for OpTransaction {
    fn size(&self) -> usize {
        match &self.0 {
            OpTypedTransaction::Legacy(tx) => tx.size(),
            OpTypedTransaction::Eip2930(tx) => tx.size(),
            OpTypedTransaction::Eip1559(tx) => tx.size(),
            OpTypedTransaction::Eip7702(tx) => tx.size(),
            OpTypedTransaction::Deposit(tx) => tx.size(),
        }
    }
}
