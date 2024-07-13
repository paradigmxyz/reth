use crate::Compact;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use op_alloy_consensus::TxDeposit as AlloyDeposit;
use reth_codecs_derive::main_codec;

/// Deposit transactions, also known as deposits are initiated on L1, and executed on L2.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
struct Deposit {
    /// Hash that uniquely identifies the source of the deposit.
    source_hash: B256,
    /// The address of the sender account.
    from: Address,
    /// The address of the recipient account, or the null (zero-length) address if the deposited
    /// transaction is a contract creation.
    to: TxKind,
    /// The ETH value to mint on L2.
    mint: Option<u128>,
    ///  The ETH value to send to the recipient account.
    value: U256,
    /// The gas limit for the L2 transaction.
    gas_limit: u64,
    /// Field indicating if this transaction is exempt from the L2 gas limit.
    is_system_transaction: bool,
    /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
    /// Some).
    input: Bytes,
}

impl Compact for AlloyDeposit {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let deposit = Deposit {
            source_hash: self.source_hash,
            from: self.from,
            to: self.to,
            mint: self.mint,
            value: self.value,
            gas_limit: self.gas_limit as u64,
            is_system_transaction: self.is_system_transaction,
            input: self.input,
        };
        deposit.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (deposit, _) = Deposit::from_compact(buf, len);
        let alloy_withdrawal = Self {
            source_hash: deposit.source_hash,
            from: deposit.from,
            to: deposit.to,
            mint: deposit.mint,
            value: deposit.value,
            gas_limit: deposit.gas_limit as u128,
            is_system_transaction: deposit.is_system_transaction,
            input: deposit.input,
        };
        (alloy_withdrawal, buf)
    }
}
