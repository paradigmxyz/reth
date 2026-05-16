//! Contains the Ethereum transaction type identifier.

use crate::TxType;
use core::fmt;

#[allow(clippy::derivable_impls)]
impl Default for TxType {
    fn default() -> Self {
        Self::Legacy
    }
}

impl TxType {
    /// Returns true if the transaction type is Legacy.
    #[inline]
    pub const fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }

    /// Returns true if the transaction type is EIP-2930.
    #[inline]
    pub const fn is_eip2930(&self) -> bool {
        matches!(self, Self::Eip2930)
    }

    /// Returns true if the transaction type is EIP-1559.
    #[inline]
    pub const fn is_eip1559(&self) -> bool {
        matches!(self, Self::Eip1559)
    }

    /// Returns true if the transaction type is EIP-4844.
    #[inline]
    pub const fn is_eip4844(&self) -> bool {
        matches!(self, Self::Eip4844)
    }

    /// Returns true if the transaction type is EIP-7702.
    #[inline]
    pub const fn is_eip7702(&self) -> bool {
        matches!(self, Self::Eip7702)
    }

    /// Returns true if the transaction type has dynamic fee.
    #[inline]
    pub const fn is_dynamic_fee(&self) -> bool {
        matches!(self, Self::Eip1559 | Self::Eip4844 | Self::Eip7702)
    }
}

impl fmt::Display for TxType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy => write!(f, "Legacy"),
            Self::Eip2930 => write!(f, "EIP-2930"),
            Self::Eip1559 => write!(f, "EIP-1559"),
            Self::Eip4844 => write!(f, "EIP-4844"),
            Self::Eip7702 => write!(f, "EIP-7702"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_u8_id() {
        assert_eq!(TxType::Legacy, TxType::Legacy as u8);
        assert_eq!(TxType::Eip2930, TxType::Eip2930 as u8);
        assert_eq!(TxType::Eip1559, TxType::Eip1559 as u8);
        assert_eq!(TxType::Eip7702, TxType::Eip7702 as u8);
        assert_eq!(TxType::Eip4844, TxType::Eip4844 as u8);
    }
}
