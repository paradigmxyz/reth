use reth_codecs::{use_scale, Compact};

/// Transaction Type
#[use_scale]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
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

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        (
            match len {
                0 => TxType::Legacy,
                1 => TxType::EIP2930,
                _ => TxType::EIP1559,
            },
            buf,
        )
    }
}
