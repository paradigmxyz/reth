use crate::{Address, Bytes, Signature, TransactionKind, TxType, H256};
use bytes::Buf;
use reth_codecs::{main_codec, Compact};
use reth_rlp::{length_of_length, Decodable, DecodeError, Encodable, Header, EMPTY_STRING_CODE};
use std::mem;

/// A versioned byte sequence to enable the protocol to upgrade the deposit transaction type without
/// changing the transaction type selector.
pub const DEPOSIT_VERSION: u8 = 0;

/// Deposited transactions, also known as deposits are transactions which are initiated on L1, and
/// executed on L2. This document outlines a new transaction type for deposits. It also describes
/// how deposits are initiated on L1, along with the authorization and validation conditions on L2.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxDeposit {
    /// Hash that uniquely identifies the source of the deposit.
    pub source_hash: H256,
    /// The address of the sender account.
    pub from: Address,
    /// The address of the recipient account, or the null (zero-length) address if the deposited
    /// transaction is a contract creation.
    pub to: TransactionKind,
    /// The ETH value to mint on L2.
    pub mint: Option<u128>,
    ///  The ETH value to send to the recipient account.
    pub value: u128,
    /// The gas limit for the L2 transaction.
    pub gas_limit: u64,
    /// Field indicating if this transaction is exempt from the L2 gas limit.
    pub is_system_transaction: bool,
    /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
    /// Some).
    pub input: Bytes,
}

impl TxDeposit {
    /// Calculates a heuristic for the in-memory size of the [TxDeposit] transaction.
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<H256>() + // source_hash
        mem::size_of::<Address>() + // from
        self.to.size() + // to
        mem::size_of::<Option<u128>>() + // mint
        mem::size_of::<u128>() + // value
        mem::size_of::<u64>() + // gas_limit
        mem::size_of::<bool>() + // is_system_transaction
        self.input.len() // input
    }

    /// Decodes the inner [TxDeposit] fields from RLP bytes.
    ///
    /// NOTE: This assumes a RLP header has already been decoded, and _just_ decodes the following
    /// RLP fields in the following order:
    ///
    /// - `source_hash`
    /// - `from`
    /// - `to`
    /// - `mint`
    /// - `value`
    /// - `input`
    /// - `gas_limit`
    /// - `is_system_transaction`
    pub fn decode_inner(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            source_hash: Decodable::decode(buf)?,
            from: Decodable::decode(buf)?,
            to: Decodable::decode(buf)?,
            mint: if *buf.first().ok_or(DecodeError::InputTooShort)? == EMPTY_STRING_CODE {
                buf.advance(1);
                None
            } else {
                Some(Decodable::decode(buf)?)
            },
            value: Decodable::decode(buf)?,
            input: Decodable::decode(buf)?,
            gas_limit: Decodable::decode(buf)?,
            is_system_transaction: Decodable::decode(buf)?,
        })
    }

    /// Outputs the length of the transaction's fields, without a RLP header or length of the
    /// eip155 fields.
    pub(crate) fn fields_len(&self) -> usize {
        let mut len = 0;
        len += self.source_hash.length();
        len += self.from.length();
        len += self.to.length();
        len += self.mint.map_or(1, |mint| mint.length());
        len += self.value.length();
        len += self.input.0.length();
        len += self.gas_limit.length();
        len += self.is_system_transaction.length();
        len
    }

    /// Encodes only the transaction's fields into the desired buffer, without a RLP header.
    /// <https://github.com/ethereum-optimism/optimism/blob/develop/specs/deposits.md#the-deposited-transaction-type>
    pub(crate) fn encode_fields(&self, out: &mut dyn bytes::BufMut) {
        self.source_hash.encode(out);
        self.from.encode(out);
        self.to.encode(out);
        if let Some(mint) = self.mint {
            mint.encode(out);
        } else {
            out.put_u8(EMPTY_STRING_CODE);
        }
        self.value.encode(out);
        self.input.encode(out);
        self.gas_limit.encode(out);
        self.is_system_transaction.encode(out);
    }

    /// Inner encoding function that is used for both rlp [`Encodable`] trait and for calculating
    /// hash that for eip2718 does not require rlp header
    pub(crate) fn encode_with_signature(
        &self,
        signature: &Signature,
        out: &mut dyn bytes::BufMut,
        with_header: bool,
    ) {
        let payload_length = self.fields_len() + signature.payload_len();
        if with_header {
            Header {
                list: false,
                payload_length: 1 + 1 + length_of_length(payload_length) + payload_length,
            }
            .encode(out);
        }
        out.put_u8(self.tx_type() as u8);
        out.put_u8(DEPOSIT_VERSION);
        let header = Header { list: true, payload_length };
        header.encode(out);
        self.encode_fields(out);
        signature.encode(out);
    }

    /// Output the length of the RLP signed transaction encoding. This encodes with a RLP header.
    pub(crate) fn payload_len_with_signature(&self, signature: &Signature) -> usize {
        let payload_length = self.fields_len() + signature.payload_len();
        // 'tx type byte length' + 'version byte' + 'header length' + 'payload length'
        let len = 1 + 1 + length_of_length(payload_length) + payload_length;
        length_of_length(len) + len
    }

    /// Get the transaction type
    pub(crate) fn tx_type(&self) -> TxType {
        TxType::DEPOSIT
    }
}
