use crate::{Bytes, ChainId, TransactionKind};
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
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    pub value: u128,
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
        mem::size_of::<u128>() + // value
        self.input.len() // input
    }
}

#[cfg(test)]
mod tests {
    use super::TxLegacy;
    use crate::{
        transaction::{signature::Signature, TransactionKind},
        Address, Transaction, TransactionSigned, H256, U256,
    };

    #[test]
    fn recover_signer_legacy() {
        use crate::hex_literal::hex;

        let signer: Address = hex!("398137383b3d25c92898c656696e41950e47316b").into();
        let hash: H256 =
            hex!("bb3a336e3f823ec18197f1e13ee875700f08f03e2cab75f0d0b118dabb44cba0").into();

        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce: 0x18,
            gas_price: 0xfa56ea00,
            gas_limit: 119902,
            to: TransactionKind::Call( hex!("06012c8cf97bead5deae237070f9587f8e7a266d").into()),
            value: 0x1c6bf526340000u64.into(),
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
