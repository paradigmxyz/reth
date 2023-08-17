use crate::{Address, Bytes, TransactionKind, H256};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{Encodable, EMPTY_STRING_CODE};
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

    /// Outputs the length of the transaction's fields, without a RLP header or length of the
    /// eip155 fields.
    pub(crate) fn fields_len(&self) -> usize {
        let mut len = 0;
        len += self.source_hash.length();
        len += self.from.length();
        len += self.to.length();
        len += self.mint.map_or(1, |mint| mint.length());
        len += self.value.length();
        len += self.gas_limit.length();
        len += self.is_system_transaction.length();
        len += self.input.0.length();
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
        self.gas_limit.encode(out);
        self.is_system_transaction.encode(out);
        self.input.encode(out);
    }
}

#[cfg(test)]
mod tests {
    use reth_rlp::Decodable;
    use revm_primitives::hex_literal::hex;

    use crate::{Bytes, TransactionSigned};

    #[test]
    fn test_rlp_decode() {
        let bytes = hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240");

        // let tx = TransactionSigned::decode_enveloped(Bytes::from(&bytes[..])).unwrap();
        let tx = TransactionSigned::decode(&mut &bytes[..]).unwrap();
        dbg!(&tx);
    }
}
