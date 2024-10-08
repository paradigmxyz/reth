//! A signed Optimism transaction.

use alloy_consensus::{EncodableSignature, TxEip1559, TxEip2930, TxEip4844};
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718};
use alloy_primitives::{keccak256, Address, Parity, TxHash, B256};
use alloy_rlp::{Buf, Decodable as _, Header};
use derive_more::{AsRef, Constructor, Deref};
use op_alloy_consensus::{OpTypedTransaction, TxDeposit};
use reth_primitives::TxType;
use reth_primitives_traits::{Signature, SignedTransaction};
use serde::{Deserialize, Serialize};

use crate::OpSignature;

/// Signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize, Constructor)]
pub struct OpTransactionSigned {
    /// Transaction hash
    pub hash: TxHash,
    /// The transaction signature values
    pub signature: OpSignature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: OpTypedTransaction,
}

impl SignedTransaction for OpTransactionSigned {
    type Transaction = OpTypedTransaction;
    type Signature = OpSignature;

    fn tx_hash(&self) -> &TxHash {
        &self.hash
    }

    fn transaction(&self) -> &Self::Transaction {
        &self.transaction
    }

    fn signature(&self) -> &Self::Signature {
        &self.signature
    }

    fn recover_signer(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        self.signature.recover_signer(signature_hash)
    }

    fn recover_signer_unchecked(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        self.signature.recover_signer_unchecked(signature_hash)
    }

    fn payload_len_inner(&self) -> usize {
        match &self.transaction {
            OpTypedTransaction::Legacy(legacy_tx) => legacy_tx
                .encoded_len_with_signature(&self.signature.with_eip155_parity(legacy_tx.chain_id)),
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, true)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, true)
            }
            OpTypedTransaction::Eip4844(blob_tx) => {
                blob_tx.encoded_len_with_signature(&self.signature, true)
            }
            /*OpTypedTransaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, true)
            }*/
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.encoded_len(true),
        }
    }

    fn length_without_header(&self) -> usize {
        // method computes the payload len without a RLP header
        match &self.transaction {
            OpTypedTransaction::Legacy(legacy_tx) => legacy_tx
                .encoded_len_with_signature(&self.signature.with_eip155_parity(legacy_tx.chain_id)),
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Eip4844(blob_tx) => {
                blob_tx.encoded_len_with_signature(&self.signature, false)
            }
            /*OpTypedTransaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, false)
            }*/
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.encoded_len(false),
        }
    }

    fn from_transaction_and_signature(
        transaction: Self::Transaction,
        signature: Self::Signature,
    ) -> Self {
        let mut initial_tx = Self { transaction, hash: Default::default(), signature };
        initial_tx.hash = initial_tx.recalculate_hash();
        initial_tx
    }

    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }
}

impl alloy_rlp::Encodable for OpTransactionSigned {
    /// See [`alloy_rlp::Encodable`] impl for
    /// [`TransactionSigned`](reth_primitives::TransactionSigned).
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !self.is_legacy() {
            payload_length += Header { list: false, payload_length }.length();
        }

        payload_length
    }
}

impl alloy_rlp::Decodable for OpTransactionSigned {
    /// See [`alloy_rlp::Decodable`] impl for
    /// [`TransactionSigned`](reth_primitives::TransactionSigned).
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for OpTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        match self.transaction.tx_type() {
            TxType::Legacy => None,
            tx_type => Some(tx_type as u8),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            OpTypedTransaction::Legacy(legacy_tx) => legacy_tx
                .encoded_len_with_signature(&self.signature.with_eip155_parity(legacy_tx.chain_id)),
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Eip4844(blob_tx) => {
                blob_tx.encoded_len_with_signature(&self.signature, false)
            }
            /*OpTypedTransaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, false)
            }*/
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.encoded_len(false),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.transaction.encode_with_signature(&self.signature, out, false)
    }
}

impl Decodable2718 for OpTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            TxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            TxType::Eip2930 => {
                let (tx, signature, hash) = TxEip2930::decode_signed_fields(buf)?.into_parts();
                Ok(Self::new(OpTypedTransaction::Eip2930(tx), signature.into(), hash))
            }
            TxType::Eip1559 => {
                let (tx, signature, hash) = TxEip1559::decode_signed_fields(buf)?.into_parts();
                Ok(Self::new(OpTypedTransaction::Eip1559(tx), signature.into(), hash))
            }
            TxType::Eip7702 => {
                todo!()
                /*let (tx, signature, hash) = TxEip7702::decode_signed_fields(buf)?.into_parts();
                Ok(Self { transaction: OpTypedTransaction::Eip7702(tx), signature, hash })*/
            }
            TxType::Eip4844 => {
                let (tx, signature, hash) = TxEip4844::decode_signed_fields(buf)?.into_parts();
                Ok(Self::new(OpTypedTransaction::Eip4844(tx), signature.into(), hash))
            }
            TxType::Deposit => Ok(Self::from_transaction_and_signature(
                OpTypedTransaction::Deposit(TxDeposit::decode(buf)?),
                OpSignature::optimism_deposit_tx_signature(),
            )),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        // keep this around so we can use it to calculate the hash
        let original_encoding_without_header = *buf;

        let tx_type = *buf.first().ok_or(alloy_rlp::Error::InputTooShort)?;
        buf.advance(1);

        // decode the list header for the rest of the transaction
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::Custom("typed tx fields must be encoded as a list").into())
        }

        let remaining_len = buf.len();

        // length of tx encoding = tx type byte (size = 1) + length of header + payload length
        let tx_length = 1 + header.length() + header.payload_length;

        // decode common fields
        let Ok(tx_type) = TxType::try_from(tx_type) else {
            return Err(alloy_rlp::Error::Custom("unsupported typed transaction type").into())
        };

        let transaction = match tx_type {
            TxType::Eip2930 => OpTypedTransaction::Eip2930(TxEip2930::decode_fields(buf)?),
            TxType::Eip1559 => OpTypedTransaction::Eip1559(TxEip1559::decode_fields(buf)?),
            TxType::Eip4844 => OpTypedTransaction::Eip4844(TxEip4844::decode_fields(buf)?),
            TxType::Eip7702 => {
                todo!()
                //OpTypedTransaction::Eip7702(TxEip7702::decode_fields(buf)?)
            }
            TxType::Deposit => OpTypedTransaction::Deposit(TxDeposit::decode_fields(buf)?),
            TxType::Legacy => {
                return Err(alloy_rlp::Error::Custom("unexpected legacy tx type").into())
            }
        };

        let signature = if tx_type == TxType::Deposit {
            OpSignature::optimism_deposit_tx_signature()
        } else {
            OpSignature::decode_rlp_vrs(buf)?
        };

        if !matches!(signature.v(), Parity::Parity(_)) {
            return Err(alloy_rlp::Error::Custom("invalid parity for typed transaction").into())
        }

        let bytes_consumed = remaining_len - buf.len();
        if bytes_consumed != header.payload_length {
            return Err(alloy_rlp::Error::UnexpectedLength.into())
        }

        let hash = keccak256(&original_encoding_without_header[..tx_length]);
        let signed = Self { transaction, hash, signature };
        Ok(signed)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for OpTransactionSigned {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[allow(unused_mut)]
        let mut transaction = OpTypedTransaction::arbitrary(u)?;
        let mut signature = alloy_primitives::Signature::arbitrary(u)?;

        signature = if matches!(transaction, OpTypedTransaction::Legacy(_)) {
            if let Some(chain_id) = transaction.chain_id() {
                signature.with_chain_id(chain_id)
            } else {
                signature.with_parity(alloy_primitives::Parity::NonEip155(bool::arbitrary(u)?))
            }
        } else {
            signature.with_parity_bool()
        };

        // Both `Some(0)` and `None` values are encoded as empty string byte. This introduces
        // ambiguity in roundtrip tests. Patch the mint value of deposit transaction here, so that
        // it's `None` if zero.
        if let OpTypedTransaction::Deposit(ref mut tx_deposit) = transaction {
            if tx_deposit.mint == Some(0) {
                tx_deposit.mint = None;
            }
        }

        let signature = if transaction.is_deposit() {
            OpSignature::optimism_deposit_tx_signature()
        } else {
            signature.into()
        };

        Ok(Self::from_transaction_and_signature(transaction, signature))
    }
}
