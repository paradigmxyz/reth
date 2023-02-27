use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

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
    /// OP Deposit transaction.
    #[cfg(feature = "optimism")]
    DEPOSIT = 126_isize,
}

impl From<TxType> for u8 {
    fn from(value: TxType) -> Self {
        match value {
            TxType::Legacy => 0,
            TxType::EIP2930 => 1,
            TxType::EIP1559 => 2,
            #[cfg(feature = "optimism")]
            TxType::DEPOSIT => 126,
        }
    }
}

impl Compact for TxType {
    fn to_compact(self, _: &mut impl bytes::BufMut) -> usize {
        match self {
            TxType::Legacy => 0,
            TxType::EIP2930 => 1,
            TxType::EIP1559 => 2,
            #[cfg(feature = "optimism")]
            TxType::DEPOSIT => 126,
        }
    }

    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        (
            match identifier {
                0 => TxType::Legacy,
                1 => TxType::EIP2930,
                2 => TxType::EIP1559,
                #[cfg(feature = "optimism")]
                126 => TxType::DEPOSIT,
                _ => panic!("unknown transaction type {identifier}"),
            },
            buf,
        )
    }
}
