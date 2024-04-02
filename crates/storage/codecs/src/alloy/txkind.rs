//! Native Compact codec impl for primitive alloy [TxKind].

use crate::Compact;
use alloy_primitives::{Address, TxKind};

impl Compact for TxKind {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match self {
            TxKind::Create => 0,
            TxKind::Call(address) => {
                address.to_compact(buf);
                1
            }
        }
    }
    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        match identifier {
            0 => (TxKind::Create, buf),
            1 => {
                let (addr, buf) = Address::from_compact(buf, buf.len());
                (addr.into(), buf)
            }
            _ => {
                unreachable!("Junk data in database: unknown TransactionKind variant",)
            }
        }
    }
}
