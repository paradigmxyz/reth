//! A signed Optimism transaction.

use alloy_consensus::{TxEip1559, TxEip2930, TxEip4844, TxEip7702};
use alloy_primitives::{keccak256, Address, Parity, Signature, TxHash};
use alloy_rlp::{Buf, Header};
use derive_more::{AsRef, Deref};
use reth_primitives::{
    optimism_deposit_tx_signature,
    transaction::{recover_signer, recover_signer_unchecked, with_eip155_parity},
    SignedTransaction, Transaction, TxDeposit, TxType,
};
use serde::{Deserialize, Serialize};

/// Signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize)]
pub struct OpTransactionSigned {
    /// Transaction hash
    pub hash: TxHash,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: Transaction,
}

impl SignedTransaction for OpTransactionSigned {
    fn recover_signer(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    fn recover_signer_unchecked(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        recover_signer_unchecked(&self.signature, signature_hash)
    }

    fn payload_len_inner(&self) -> usize {
        match &self.transaction {
            Transaction::Legacy(legacy_tx) => legacy_tx.encoded_len_with_signature(
                &with_eip155_parity(&self.signature, legacy_tx.chain_id),
            ),
            Transaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, true)
            }
            Transaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, true)
            }
            Transaction::Eip4844(blob_tx) => {
                blob_tx.encoded_len_with_signature(&self.signature, true)
            }
            Transaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, true)
            }
            Transaction::Deposit(deposit_tx) => deposit_tx.encoded_len(true),
        }
    }

    fn decode_enveloped_typed_transaction(data: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // keep this around so we can use it to calculate the hash
        let original_encoding_without_header = *data;

        let tx_type = *data.first().ok_or(alloy_rlp::Error::InputTooShort)?;
        data.advance(1);

        // decode the list header for the rest of the transaction
        let header = Header::decode(data)?;
        if !header.list {
            return Err(alloy_rlp::Error::Custom("typed tx fields must be encoded as a list"))
        }

        let remaining_len = data.len();

        // length of tx encoding = tx type byte (size = 1) + length of header + payload length
        let tx_length = 1 + header.length() + header.payload_length;

        // decode common fields
        let Ok(tx_type) = TxType::try_from(tx_type) else {
            return Err(alloy_rlp::Error::Custom("unsupported typed transaction type"))
        };

        let transaction = match tx_type {
            TxType::Eip2930 => Transaction::Eip2930(TxEip2930::decode_fields(data)?),
            TxType::Eip1559 => Transaction::Eip1559(TxEip1559::decode_fields(data)?),
            TxType::Eip4844 => Transaction::Eip4844(TxEip4844::decode_fields(data)?),
            TxType::Eip7702 => Transaction::Eip7702(TxEip7702::decode_fields(data)?),

            TxType::Deposit => Transaction::Deposit(TxDeposit::decode_fields(data)?),
            TxType::Legacy => return Err(alloy_rlp::Error::Custom("unexpected legacy tx type")),
        };

        let signature = if tx_type == TxType::Deposit {
            optimism_deposit_tx_signature()
        } else {
            Signature::decode_rlp_vrs(data)?
        };

        if !matches!(signature.v(), Parity::Parity(_)) {
            return Err(alloy_rlp::Error::Custom("invalid parity for typed transaction"));
        }

        let bytes_consumed = remaining_len - data.len();
        if bytes_consumed != header.payload_length {
            return Err(alloy_rlp::Error::UnexpectedLength)
        }

        let hash = keccak256(&original_encoding_without_header[..tx_length]);
        let signed = Self { transaction, hash, signature };
        Ok(signed)
    }

    fn length_without_header(&self) -> usize {
        // method computes the payload len without a RLP header
        match &self.transaction {
            Transaction::Legacy(legacy_tx) => legacy_tx.encoded_len_with_signature(
                &with_eip155_parity(&self.signature, legacy_tx.chain_id),
            ),
            Transaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Eip4844(blob_tx) => {
                blob_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Deposit(deposit_tx) => deposit_tx.encoded_len(false),
        }
    }
}
