use super::access_list::AccessList;
use crate::{Bytes, ChainId, TransactionKind};
use reth_codecs::{main_codec, Compact};
use std::mem;

/// A transaction with a priority fee ([EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)).
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxEip1559 {
    /// Added as EIP-pub 155: Simple replay attack protection
    pub chain_id: u64,
    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    pub nonce: u64,
    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    pub gas_limit: u64,
    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasFeeCap`
    pub max_fee_per_gas: u128,
    /// Max Priority fee that transaction is paying
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasTipCap`
    pub max_priority_fee_per_gas: u128,
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

impl TxEip1559 {
    /// Calculates a heuristic for the in-memory size of the [TxEip1559] transaction.
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<ChainId>() + // chain_id
        mem::size_of::<u64>() + // nonce
        mem::size_of::<u64>() + // gas_limit
        mem::size_of::<u128>() + // max_fee_per_gas
        mem::size_of::<u128>() + // max_priority_fee_per_gas
        self.to.size() + // to
        mem::size_of::<u128>() + // value
        self.access_list.size() + // access_list
        self.input.len() // input
    }
}

#[cfg(test)]
mod tests {
    use super::TxEip1559;
    use crate::{
        transaction::{signature::Signature, TransactionKind},
        AccessList, Address, Transaction, TransactionSigned, H256, U256,
    };
    use std::str::FromStr;

    #[test]
    fn recover_signer_eip1559() {
        use crate::hex_literal::hex;

        let signer: Address = hex!("dd6b8b3dc6b7ad97db52f08a275ff4483e024cea").into();
        let hash: H256 =
            hex!("0ec0b6a2df4d87424e5f6ad2a654e27aaeb7dac20ae9e8385cc09087ad532ee0").into();

        let tx = Transaction::Eip1559( TxEip1559 {
            chain_id: 1,
            nonce: 0x42,
            gas_limit: 44386,
            to: TransactionKind::Call( hex!("6069a6c32cf691f5982febae4faf8a6f3ab2f0f6").into()),
            value: 0,
            input:  hex!("a22cb4650000000000000000000000005eee75727d804a2b13038928d36f8b188945a57a0000000000000000000000000000000000000000000000000000000000000000").into(),
            max_fee_per_gas: 0x4a817c800,
            max_priority_fee_per_gas: 0x3b9aca00,
            access_list: AccessList::default(),
        });

        let sig = Signature {
            r: U256::from_str("0x840cfc572845f5786e702984c2a582528cad4b49b2a10b9db1be7fca90058565")
                .unwrap(),
            s: U256::from_str("0x25e7109ceb98168d95b09b18bbf6b685130e0562f233877d492b94eee0c5b6d1")
                .unwrap(),
            odd_y_parity: false,
        };

        let signed_tx = TransactionSigned::from_transaction_and_signature(tx, sig);
        assert_eq!(signed_tx.hash(), hash, "Expected same hash");
        assert_eq!(signed_tx.recover_signer(), Some(signer), "Recovering signer should pass.");
    }
}
