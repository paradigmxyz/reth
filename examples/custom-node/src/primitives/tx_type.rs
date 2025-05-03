use alloy_primitives::bytes::{Buf, BufMut};
use reth_codecs::{txtype::COMPACT_EXTENDED_IDENTIFIER_FLAG, Compact};
use serde::{Deserialize, Serialize};

pub const TRANSFER_TX_TYPE_ID: u8 = 42;

/// An enum for the custom transaction type(s)
#[repr(u8)]
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum TxTypeCustom {
    Custom = TRANSFER_TX_TYPE_ID,
}

impl From<TxTypeCustom> for u8 {
    fn from(value: TxTypeCustom) -> Self {
        value as Self
    }
}

impl Compact for TxTypeCustom {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self {
            Self::Custom => {
                buf.put_u8(TRANSFER_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
        }
    }

    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        (
            match identifier {
                COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                    let extended_identifier = buf.get_u8();
                    match extended_identifier {
                        TRANSFER_TX_TYPE_ID => Self::Custom,
                        _ => panic!("Unsupported TxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for TxType: {identifier}"),
            },
            buf,
        )
    }
}
