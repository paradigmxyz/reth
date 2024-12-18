//! Native Compact codec impl for primitive alloy [`TxKind`].

use crate::Compact;
use alloy_primitives::{Address, TxKind};

/// Identifier for [`TxKind::Create`]
const TX_KIND_TYPE_CREATE: usize = 0;

/// Identifier for [`TxKind::Call`]
const TX_KIND_TYPE_CALL: usize = 1;

impl Compact for TxKind {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match self {
            Self::Create => TX_KIND_TYPE_CREATE,
            Self::Call(address) => {
                address.to_compact(buf);
                TX_KIND_TYPE_CALL
            }
        }
    }
    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        match identifier {
            TX_KIND_TYPE_CREATE => (Self::Create, buf),
            TX_KIND_TYPE_CALL => {
                let (addr, buf) = Address::from_compact(buf, buf.len());
                (addr.into(), buf)
            }
            _ => {
                unreachable!("Junk data in database: unknown TransactionKind variant",)
            }
        }
    }
}
