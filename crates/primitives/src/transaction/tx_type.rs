use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// Identifier for legacy transaction, however [TxLegacy](crate::TxLegacy) this is technically not
/// typed.
pub const LEGACY_TX_TYPE_ID: u8 = 0;

/// Identifier for [TxEip2930](crate::TxEip2930) transaction.
pub const EIP2930_TX_TYPE_ID: u8 = 1;

/// Identifier for [TxEip1559](crate::TxEip1559) transaction.
pub const EIP1559_TX_TYPE_ID: u8 = 2;

/// Transaction Type
#[derive_arbitrary(compact)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum TxType {
    /// Legacy transaction pre EIP-2929
    #[default]
    Legacy = 0_isize,
    /// AccessList transaction
    EIP2930 = 1_isize,
    /// Transaction with Priority fee
    EIP1559 = 2_isize,
}

impl From<TxType> for u8 {
    fn from(value: TxType) -> Self {
        match value {
            TxType::Legacy => LEGACY_TX_TYPE_ID,
            TxType::EIP2930 => EIP2930_TX_TYPE_ID,
            TxType::EIP1559 => EIP1559_TX_TYPE_ID,
        }
    }
}

impl Compact for TxType {
    fn to_compact(self, _: &mut impl bytes::BufMut) -> usize {
        match self {
            TxType::Legacy => 0,
            TxType::EIP2930 => 1,
            TxType::EIP1559 => 2,
        }
    }

    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        (
            match identifier {
                0 => TxType::Legacy,
                1 => TxType::EIP2930,
                _ => TxType::EIP1559,
            },
            buf,
        )
    }
}
