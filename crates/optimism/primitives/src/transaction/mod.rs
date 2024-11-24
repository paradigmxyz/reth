//! Wrapper of [`OpTypedTransaction`], that implements reth database encoding [`Compact`].

pub mod tx_type;

use alloy_primitives::{bytes, Bytes, TxKind, Uint, B256};

use alloy_consensus::{constants::EIP7702_TX_TYPE_ID, TxLegacy};
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use derive_more::{Deref, From};
use op_alloy_consensus::{OpTypedTransaction, DEPOSIT_TX_TYPE_ID};
use reth_codecs::Compact;
use reth_primitives::transaction::{
    COMPACT_EXTENDED_IDENTIFIER_FLAG, COMPACT_IDENTIFIER_EIP1559, COMPACT_IDENTIFIER_EIP2930,
    COMPACT_IDENTIFIER_LEGACY,
};
use reth_primitives_traits::InMemorySize;

#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Deref, Hash, From)]
/// Optimistic transaction.
pub struct OpTransaction(OpTypedTransaction);

impl Default for OpTransaction {
    fn default() -> Self {
        Self(OpTypedTransaction::Legacy(TxLegacy::default()))
    }
}

impl Compact for OpTransaction {
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

    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        match identifier {
            COMPACT_IDENTIFIER_LEGACY => {
                let (tx, buf) = TxLegacy::from_compact(buf, buf.len());
                (Self(OpTypedTransaction::Legacy(tx)), buf)
            }
            COMPACT_IDENTIFIER_EIP2930 => {
                let (tx, buf) =
                    alloy_consensus::transaction::TxEip2930::from_compact(buf, buf.len());
                (Self(OpTypedTransaction::Eip2930(tx)), buf)
            }
            COMPACT_IDENTIFIER_EIP1559 => {
                let (tx, buf) =
                    alloy_consensus::transaction::TxEip1559::from_compact(buf, buf.len());
                (Self(OpTypedTransaction::Eip1559(tx)), buf)
            }
            COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                // An identifier of 3 indicates that the transaction type did not fit into
                // the backwards compatible 2 bit identifier, their transaction types are
                // larger than 2 bits (eg. 4844 and Deposit Transactions). In this case,
                // we need to read the concrete transaction type from the buffer by
                // reading the full 8 bits (single byte) and match on this transaction type.
                let identifier = buf.get_u8();
                match identifier {
                    EIP7702_TX_TYPE_ID => {
                        let (tx, buf) =
                            alloy_consensus::transaction::TxEip7702::from_compact(buf, buf.len());
                        (Self(OpTypedTransaction::Eip7702(tx)), buf)
                    }
                    DEPOSIT_TX_TYPE_ID => {
                        let (tx, buf) = op_alloy_consensus::TxDeposit::from_compact(buf, buf.len());
                        (Self(OpTypedTransaction::Deposit(tx)), buf)
                    }
                    _ => unreachable!(
                        "Junk data in database: unknown Transaction variant: {identifier}"
                    ),
                }
            }
            _ => unreachable!("Junk data in database: unknown Transaction variant: {identifier}"),
        }
    }
}

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
