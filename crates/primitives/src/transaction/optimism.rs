use crate::{Address, Bytes, TransactionKind, TxType, B256, U256};
use alloy_rlp::{
    length_of_length, Decodable, Encodable, Error as DecodeError, Header, EMPTY_STRING_CODE,
};
use bytes::Buf;
use reth_codecs::{main_codec, Compact};
use std::mem;

/// Deposit transactions, also known as deposits are initiated on L1, and executed on L2.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxDeposit {
    /// Hash that uniquely identifies the source of the deposit.
    pub source_hash: B256,
    /// The address of the sender account.
    pub from: Address,
    /// The address of the recipient account, or the null (zero-length) address if the deposited
    /// transaction is a contract creation.
    pub to: TransactionKind,
    /// The ETH value to mint on L2.
    pub mint: Option<u128>,
    ///  The ETH value to send to the recipient account.
    pub value: U256,
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
        mem::size_of::<B256>() + // source_hash
        mem::size_of::<Address>() + // from
        self.to.size() + // to
        mem::size_of::<Option<u128>>() + // mint
        mem::size_of::<U256>() + // value
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
    /// - `gas_limit`
    /// - `is_system_transaction`
    /// - `input`
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
            gas_limit: Decodable::decode(buf)?,
            is_system_transaction: Decodable::decode(buf)?,
            input: Decodable::decode(buf)?,
        })
    }

    /// Outputs the length of the transaction's fields, without a RLP header or length of the
    /// eip155 fields.
    pub(crate) fn fields_len(&self) -> usize {
        self.source_hash.length() +
            self.from.length() +
            self.to.length() +
            self.mint.map_or(1, |mint| mint.length()) +
            self.value.length() +
            self.gas_limit.length() +
            self.is_system_transaction.length() +
            self.input.0.length()
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
        self.gas_limit.encode(out);
        self.is_system_transaction.encode(out);
        self.input.encode(out);
    }

    /// Inner encoding function that is used for both rlp [`Encodable`] trait and for calculating
    /// hash that for eip2718 does not require rlp header
    pub(crate) fn encode(&self, out: &mut dyn bytes::BufMut, with_header: bool) {
        let payload_length = self.fields_len();
        if with_header {
            Header {
                list: false,
                payload_length: 1 + length_of_length(payload_length) + payload_length,
            }
            .encode(out);
        }
        out.put_u8(self.tx_type() as u8);
        let header = Header { list: true, payload_length };
        header.encode(out);
        self.encode_fields(out);
    }

    /// Output the length of the RLP signed transaction encoding. This encodes with a RLP header.
    pub(crate) fn payload_len(&self) -> usize {
        let payload_length = self.fields_len();
        // 'tx type' + 'header length' + 'payload length'
        let len = 1 + length_of_length(payload_length) + payload_length;
        length_of_length(len) + len
    }

    pub(crate) fn payload_len_without_header(&self) -> usize {
        let payload_length = self.fields_len();
        // 'transaction type byte length' + 'header length' + 'payload length'
        1 + length_of_length(payload_length) + payload_length
    }

    /// Get the transaction type
    pub(crate) fn tx_type(&self) -> TxType {
        TxType::Deposit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{revm_primitives::hex_literal::hex, TransactionSigned};
    use bytes::BytesMut;

    #[test]
    fn test_rlp_roundtrip() {
        let bytes = Bytes::from_static(&hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240"));

        let tx_a = TransactionSigned::decode_enveloped(&mut bytes.as_ref()).unwrap();
        let tx_b = TransactionSigned::decode(&mut &bytes[..]).unwrap();

        let mut buf_a = BytesMut::default();
        tx_a.encode_enveloped(&mut buf_a);
        assert_eq!(&buf_a[..], &bytes[..]);

        let mut buf_b = BytesMut::default();
        tx_b.encode_enveloped(&mut buf_b);
        assert_eq!(&buf_b[..], &bytes[..]);
    }

    #[test]
    fn test_encode_decode_fields() {
        let original = TxDeposit {
            source_hash: B256::default(),
            from: Address::default(),
            to: TransactionKind::default(),
            mint: Some(100),
            value: U256::default(),
            gas_limit: 50000,
            is_system_transaction: true,
            input: Bytes::default(),
        };

        let mut buffer = BytesMut::new();
        original.encode_fields(&mut buffer);
        let decoded = TxDeposit::decode_inner(&mut &buffer[..]).expect("Failed to decode");

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_encode_with_and_without_header() {
        let tx_deposit = TxDeposit {
            source_hash: B256::default(),
            from: Address::default(),
            to: TransactionKind::default(),
            mint: Some(100),
            value: U256::default(),
            gas_limit: 50000,
            is_system_transaction: true,
            input: Bytes::default(),
        };

        let mut buffer_with_header = BytesMut::new();
        tx_deposit.encode(&mut buffer_with_header, true);

        let mut buffer_without_header = BytesMut::new();
        tx_deposit.encode(&mut buffer_without_header, false);

        assert!(buffer_with_header.len() > buffer_without_header.len());
    }

    #[test]
    fn test_payload_length() {
        let tx_deposit = TxDeposit {
            source_hash: B256::default(),
            from: Address::default(),
            to: TransactionKind::default(),
            mint: Some(100),
            value: U256::default(),
            gas_limit: 50000,
            is_system_transaction: true,
            input: Bytes::default(),
        };

        let total_len = tx_deposit.payload_len();
        let len_without_header = tx_deposit.payload_len_without_header();

        assert!(total_len > len_without_header);
    }
}
