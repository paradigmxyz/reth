use crate::{U64, U8};
use bytes::Buf;
use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// Identifier for legacy transaction, however [TxLegacy](crate::TxLegacy) this is technically not
/// typed.
pub const LEGACY_TX_TYPE_ID: u8 = 0;

/// Identifier for [TxEip2930](crate::TxEip2930) transaction.
pub const EIP2930_TX_TYPE_ID: u8 = 1;

/// Identifier for [TxEip1559](crate::TxEip1559) transaction.
pub const EIP1559_TX_TYPE_ID: u8 = 2;

/// Identifier for [TxEip4844](crate::TxEip4844) transaction.
pub const EIP4844_TX_TYPE_ID: u8 = 3;

/// Identifier for [TxDeposit](crate::TxDeposit) transaction.
#[cfg(feature = "optimism")]
pub const DEPOSIT_TX_TYPE_ID: u8 = 126;

/// Transaction Type
///
/// Currently being used as 2-bit type when encoding it to [`Compact`] on
/// [`crate::TransactionSignedNoHash`]. Adding more transaction types will break the codec and
/// database format.
///
/// Other required changes when adding a new type can be seen on [PR#3953](https://github.com/paradigmxyz/reth/pull/3953/files).
#[derive_arbitrary(compact)]
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize, Hash,
)]
pub enum TxType {
    /// Legacy transaction pre EIP-2929
    #[default]
    Legacy = 0_isize,
    /// AccessList transaction
    Eip2930 = 1_isize,
    /// Transaction with Priority fee
    Eip1559 = 2_isize,
    /// Shard Blob Transactions - EIP-4844
    Eip4844 = 3_isize,
    /// Optimism Deposit transaction.
    #[cfg(feature = "optimism")]
    Deposit = 126_isize,
}

impl TxType {
    /// The max type reserved by an EIP.
    pub const MAX_RESERVED_EIP: TxType = Self::Eip4844;

    /// Check if the transaction type has an access list.
    pub const fn has_access_list(&self) -> bool {
        match self {
            TxType::Legacy => false,
            TxType::Eip2930 | TxType::Eip1559 | TxType::Eip4844 => true,
            #[cfg(feature = "optimism")]
            TxType::Deposit => false,
        }
    }
}

impl From<TxType> for u8 {
    fn from(value: TxType) -> Self {
        match value {
            TxType::Legacy => LEGACY_TX_TYPE_ID,
            TxType::Eip2930 => EIP2930_TX_TYPE_ID,
            TxType::Eip1559 => EIP1559_TX_TYPE_ID,
            TxType::Eip4844 => EIP4844_TX_TYPE_ID,
            #[cfg(feature = "optimism")]
            TxType::Deposit => DEPOSIT_TX_TYPE_ID,
        }
    }
}

impl From<TxType> for U8 {
    fn from(value: TxType) -> Self {
        U8::from(u8::from(value))
    }
}

impl TryFrom<u8> for TxType {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        #[cfg(feature = "optimism")]
        if value == TxType::Deposit as u8 {
            return Ok(TxType::Deposit)
        }

        if value == TxType::Legacy as u8 {
            return Ok(TxType::Legacy)
        } else if value == TxType::Eip2930 as u8 {
            return Ok(TxType::Eip2930)
        } else if value == TxType::Eip1559 as u8 {
            return Ok(TxType::Eip1559)
        } else if value == TxType::Eip4844 as u8 {
            return Ok(TxType::Eip4844)
        }

        Err("invalid tx type")
    }
}

impl TryFrom<u64> for TxType {
    type Error = &'static str;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let value: u8 = value.try_into().map_err(|_| "invalid tx type")?;
        Self::try_from(value)
    }
}

impl TryFrom<U64> for TxType {
    type Error = &'static str;

    fn try_from(value: U64) -> Result<Self, Self::Error> {
        value.to::<u64>().try_into()
    }
}

impl Compact for TxType {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match self {
            TxType::Legacy => 0,
            TxType::Eip2930 => 1,
            TxType::Eip1559 => 2,
            TxType::Eip4844 => {
                // Write the full transaction type to the buffer when encoding > 3.
                // This allows compat decoding the [TyType] from a single byte as
                // opposed to 2 bits for the backwards-compatible encoding.
                buf.put_u8(self as u8);
                3
            }
            #[cfg(feature = "optimism")]
            TxType::Deposit => {
                buf.put_u8(self as u8);
                3
            }
        }
    }

    // For backwards compatibility purposes only 2 bits of the type are encoded in the identifier
    // parameter. In the case of a 3, the full transaction type is read from the buffer as a
    // single byte.
    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        (
            match identifier {
                0 => TxType::Legacy,
                1 => TxType::Eip2930,
                2 => TxType::Eip1559,
                3 => {
                    let extended_identifier = buf.get_u8();
                    match extended_identifier {
                        EIP4844_TX_TYPE_ID => TxType::Eip4844,
                        #[cfg(feature = "optimism")]
                        DEPOSIT_TX_TYPE_ID => TxType::Deposit,
                        _ => panic!("Unsupported TxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for TxType: {identifier}"),
            },
            buf,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u64_to_tx_type() {
        // Test for Legacy transaction
        assert_eq!(TxType::try_from(U64::from(0)).unwrap(), TxType::Legacy);

        // Test for EIP2930 transaction
        assert_eq!(TxType::try_from(U64::from(1)).unwrap(), TxType::Eip2930);

        // Test for EIP1559 transaction
        assert_eq!(TxType::try_from(U64::from(2)).unwrap(), TxType::Eip1559);

        // Test for EIP4844 transaction
        assert_eq!(TxType::try_from(U64::from(3)).unwrap(), TxType::Eip4844);

        // Test for Deposit transaction
        #[cfg(feature = "optimism")]
        assert_eq!(TxType::try_from(U64::from(126)).unwrap(), TxType::Deposit);

        // For transactions with unsupported values
        assert!(TxType::try_from(U64::from(4)).is_err());
    }

    #[test]
    fn test_txtype_to_compat() {
        let cases = vec![
            (TxType::Legacy, 0, vec![]),
            (TxType::Eip2930, 1, vec![]),
            (TxType::Eip1559, 2, vec![]),
            (TxType::Eip4844, 3, vec![EIP4844_TX_TYPE_ID]),
            #[cfg(feature = "optimism")]
            (TxType::Deposit, 3, vec![DEPOSIT_TX_TYPE_ID]),
        ];

        for (tx_type, expected_identifier, expected_buf) in cases {
            let mut buf = vec![];
            let identifier = tx_type.to_compact(&mut buf);
            assert_eq!(
                identifier, expected_identifier,
                "Unexpected identifier for TxType {tx_type:?}",
            );
            assert_eq!(buf, expected_buf, "Unexpected buffer for TxType {tx_type:?}");
        }
    }

    #[test]
    fn test_txtype_from_compact() {
        let cases = vec![
            (TxType::Legacy, 0, vec![]),
            (TxType::Eip2930, 1, vec![]),
            (TxType::Eip1559, 2, vec![]),
            (TxType::Eip4844, 3, vec![EIP4844_TX_TYPE_ID]),
            #[cfg(feature = "optimism")]
            (TxType::Deposit, 3, vec![DEPOSIT_TX_TYPE_ID]),
        ];

        for (expected_type, identifier, buf) in cases {
            let (actual_type, remaining_buf) = TxType::from_compact(&buf, identifier);
            assert_eq!(actual_type, expected_type, "Unexpected TxType for identifier {identifier}",);
            assert!(
                remaining_buf.is_empty(),
                "Buffer not fully consumed for identifier {identifier}",
            );
        }
    }
}
