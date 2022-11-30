use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

/// Transaction Type
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

impl Compact for TxType {
    fn to_compact(self, _: &mut impl bytes::BufMut) -> usize {
        match self {
            TxType::Legacy => 0,
            TxType::EIP2930 => 1,
            _ => 2,
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

    fn alternative_to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        self.to_compact(buf)
    }

    fn alternative_from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        Self::from_compact(buf, len)
    }
}
