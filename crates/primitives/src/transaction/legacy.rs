use crate::{keccak256, Bytes, ChainId, Signature, TransactionKind, TxType, B256, U256};
use alloy_rlp::{length_of_length, Encodable, Header};
use bytes::BytesMut;
use reth_codecs::{main_codec, Compact};
use std::mem;

/// Legacy transaction.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxLegacy {
    /// Added as EIP-155: Simple replay attack protection
    pub chain_id: Option<ChainId>,
    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    pub nonce: u64,
    /// A scalar value equal to the number of
    /// Wei to be paid per unit of gas for all computation
    /// costs incurred as a result of the execution of this transaction; formally Tp.
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    pub gas_price: u128,
    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    pub gas_limit: u64,
    /// The 160-bit address of the message call’s recipient or, for a contract creation
    /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
    pub to: TransactionKind,
    /// A scalar value equal to the number of Wei to
    /// be transferred to the message call’s recipient or,
    /// in the case of contract creation, as an endowment
    /// to the newly created account; formally Tv.
    pub value: U256,
    /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
    /// Some). pub init: An unlimited size byte array specifying the
    /// EVM-code for the account initialisation procedure CREATE,
    /// data: An unlimited size byte array specifying the
    /// input data of the message call, formally Td.
    pub input: Bytes,
}

impl TxLegacy {
    /// Calculates a heuristic for the in-memory size of the [TxLegacy] transaction.
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<Option<ChainId>>() + // chain_id
        mem::size_of::<u64>() + // nonce
        mem::size_of::<u128>() + // gas_price
        mem::size_of::<u64>() + // gas_limit
        self.to.size() + // to
        mem::size_of::<U256>() + // value
        self.input.len() // input
    }

    /// Outputs the length of the transaction's fields, without a RLP header or length of the
    /// eip155 fields.
    pub(crate) fn fields_len(&self) -> usize {
        self.nonce.length() +
            self.gas_price.length() +
            self.gas_limit.length() +
            self.to.length() +
            self.value.length() +
            self.input.0.length()
    }

    /// Encodes only the transaction's fields into the desired buffer, without a RLP header or
    /// eip155 fields.
    pub(crate) fn encode_fields(&self, out: &mut dyn bytes::BufMut) {
        self.nonce.encode(out);
        self.gas_price.encode(out);
        self.gas_limit.encode(out);
        self.to.encode(out);
        self.value.encode(out);
        self.input.0.encode(out);
    }

    /// Inner encoding function that is used for both rlp [`Encodable`] trait and for calculating
    /// hash.
    ///
    /// This encodes the transaction as:
    /// `rlp(nonce, gas_price, gas_limit, to, value, input, v, r, s)`
    ///
    /// The `v` value is encoded according to EIP-155 if the `chain_id` is not `None`.
    pub(crate) fn encode_with_signature(&self, signature: &Signature, out: &mut dyn bytes::BufMut) {
        let payload_length =
            self.fields_len() + signature.payload_len_with_eip155_chain_id(self.chain_id);
        let header = Header { list: true, payload_length };
        header.encode(out);
        self.encode_fields(out);
        signature.encode_with_eip155_chain_id(out, self.chain_id);
    }

    /// Output the length of the RLP signed transaction encoding.
    pub(crate) fn payload_len_with_signature(&self, signature: &Signature) -> usize {
        let payload_length =
            self.fields_len() + signature.payload_len_with_eip155_chain_id(self.chain_id);
        // 'header length' + 'payload length'
        length_of_length(payload_length) + payload_length
    }

    /// Get transaction type
    pub(crate) fn tx_type(&self) -> TxType {
        TxType::Legacy
    }

    /// Encodes EIP-155 arguments into the desired buffer. Only encodes values for legacy
    /// transactions.
    ///
    /// If a `chain_id` is `Some`, this encodes the `chain_id`, followed by two zeroes, as defined
    /// by [EIP-155](https://eips.ethereum.org/EIPS/eip-155).
    pub(crate) fn encode_eip155_fields(&self, out: &mut dyn bytes::BufMut) {
        // if this is a legacy transaction without a chain ID, it must be pre-EIP-155
        // and does not need to encode the chain ID for the signature hash encoding
        if let Some(id) = self.chain_id {
            // EIP-155 encodes the chain ID and two zeroes
            id.encode(out);
            0x00u8.encode(out);
            0x00u8.encode(out);
        }
    }

    /// Outputs the length of EIP-155 fields. Only outputs a non-zero value for EIP-155 legacy
    /// transactions.
    pub(crate) fn eip155_fields_len(&self) -> usize {
        if let Some(id) = self.chain_id {
            // EIP-155 encodes the chain ID and two zeroes, so we add 2 to the length of the chain
            // ID to get the length of all 3 fields
            // len(chain_id) + (0x00) + (0x00)
            id.length() + 2
        } else {
            // this is either a pre-EIP-155 legacy transaction or a typed transaction
            0
        }
    }

    /// Encodes the legacy transaction in RLP for signing, including the EIP-155 fields if possible.
    ///
    /// If a `chain_id` is `Some`, this encodes the transaction as:
    /// `rlp(nonce, gas_price, gas_limit, to, value, input, chain_id, 0, 0)`
    ///
    /// Otherwise, this encodes the transaction as:
    /// `rlp(nonce, gas_price, gas_limit, to, value, input)`
    pub(crate) fn encode_for_signing(&self, out: &mut dyn bytes::BufMut) {
        Header { list: true, payload_length: self.fields_len() + self.eip155_fields_len() }
            .encode(out);
        self.encode_fields(out);
        self.encode_eip155_fields(out);
    }

    /// Outputs the length of the signature RLP encoding for the transaction, including the length
    /// of the EIP-155 fields if possible.
    pub(crate) fn payload_len_for_signature(&self) -> usize {
        let payload_length = self.fields_len() + self.eip155_fields_len();
        // 'header length' + 'payload length'
        length_of_length(payload_length) + payload_length
    }

    /// Outputs the signature hash of the transaction by first encoding without a signature, then
    /// hashing.
    ///
    /// See [Self::encode_for_signing] for more information on the encoding format.
    pub(crate) fn signature_hash(&self) -> B256 {
        let mut buf = BytesMut::with_capacity(self.payload_len_for_signature());
        self.encode_for_signing(&mut buf);
        keccak256(&buf)
    }
}

#[cfg(test)]
mod tests {
    use super::TxLegacy;
    use crate::{
        transaction::{signature::Signature, TransactionKind},
        Address, Transaction, TransactionSigned, B256, U256,
    };

    #[test]
    fn recover_signer_legacy() {
        use crate::hex_literal::hex;

        let signer: Address = hex!("398137383b3d25c92898c656696e41950e47316b").into();
        let hash: B256 =
            hex!("bb3a336e3f823ec18197f1e13ee875700f08f03e2cab75f0d0b118dabb44cba0").into();

        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce: 0x18,
            gas_price: 0xfa56ea00,
            gas_limit: 119902,
            to: TransactionKind::Call( hex!("06012c8cf97bead5deae237070f9587f8e7a266d").into()),
            value: U256::from(0x1c6bf526340000u64),
            input:  hex!("f7d8c88300000000000000000000000000000000000000000000000000000000000cee6100000000000000000000000000000000000000000000000000000000000ac3e1").into(),
        });

        let sig = Signature {
            r: U256::from_be_bytes(hex!(
                "2a378831cf81d99a3f06a18ae1b6ca366817ab4d88a70053c41d7a8f0368e031"
            )),
            s: U256::from_be_bytes(hex!(
                "450d831a05b6e418724436c05c155e0a1b7b921015d0fbc2f667aed709ac4fb5"
            )),
            odd_y_parity: false,
        };

        let signed_tx = TransactionSigned::from_transaction_and_signature(tx, sig);
        assert_eq!(signed_tx.hash(), hash, "Expected same hash");
        assert_eq!(signed_tx.recover_signer(), Some(signer), "Recovering signer should pass.");
    }
}
