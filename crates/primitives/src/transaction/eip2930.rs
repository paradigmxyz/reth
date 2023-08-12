use super::access_list::AccessList;
use crate::{Bytes, ChainId, TransactionKind};
use reth_codecs::{main_codec, Compact};
use std::mem;

/// Transaction with an [`AccessList`] ([EIP-2930](https://eips.ethereum.org/EIPS/eip-2930)).
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxEip2930 {
    /// Added as EIP-pub 155: Simple replay attack protection
    pub chain_id: ChainId,
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
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    pub value: u128,
    /// The accessList specifies a list of addresses and storage keys;
    /// these addresses and storage keys are added into the `accessed_addresses`
    /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
    /// A gas cost is charged, though at a discount relative to the cost of
    /// accessing outside the list.
    pub access_list: AccessList,
    /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
    /// Some). pub init: An unlimited size byte array specifying the
    /// EVM-code for the account initialisation procedure CREATE,
    /// data: An unlimited size byte array specifying the
    /// input data of the message call, formally Td.
    pub input: Bytes,
}

impl TxEip2930 {
    /// Calculates a heuristic for the in-memory size of the [TxEip2930] transaction.
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<ChainId>() + // chain_id
        mem::size_of::<u64>() + // nonce
        mem::size_of::<u128>() + // gas_price
        mem::size_of::<u64>() + // gas_limit
        self.to.size() + // to
        mem::size_of::<u128>() + // value
        self.access_list.size() + // access_list
        self.input.len() // input
    }
}

#[cfg(test)]
mod tests {
    use super::TxEip2930;
    use crate::{
        transaction::{signature::Signature, TransactionKind},
        Address, Bytes, Transaction, TransactionSigned, U256,
    };
    use bytes::BytesMut;
    use reth_rlp::{Decodable, Encodable};

    #[test]
    fn test_decode_create() {
        // tests that a contract creation tx encodes and decodes properly
        let request = Transaction::Eip2930(TxEip2930 {
            chain_id: 1u64,
            nonce: 0,
            gas_price: 1,
            gas_limit: 2,
            to: TransactionKind::Create,
            value: 3,
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
        });
        let signature = Signature { odd_y_parity: true, r: U256::default(), s: U256::default() };
        let tx = TransactionSigned::from_transaction_and_signature(request, signature);

        let mut encoded = BytesMut::new();
        tx.encode(&mut encoded);
        assert_eq!(encoded.len(), tx.length());

        let decoded = TransactionSigned::decode(&mut &*encoded).unwrap();
        assert_eq!(decoded, tx);
    }

    #[test]
    fn test_decode_call() {
        let request = Transaction::Eip2930(TxEip2930 {
            chain_id: 1u64,
            nonce: 0,
            gas_price: 1,
            gas_limit: 2,
            to: TransactionKind::Call(Address::default()),
            value: 3,
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
        });

        let signature = Signature { odd_y_parity: true, r: U256::default(), s: U256::default() };

        let tx = TransactionSigned::from_transaction_and_signature(request, signature);

        let mut encoded = BytesMut::new();
        tx.encode(&mut encoded);
        assert_eq!(encoded.len(), tx.length());

        let decoded = TransactionSigned::decode(&mut &*encoded).unwrap();
        assert_eq!(decoded, tx);
    }
}
