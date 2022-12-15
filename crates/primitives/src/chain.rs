use crate::U256;
use ethers_core::types::{ParseChainError, U64};
use reth_rlp::{Decodable, Encodable};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Either a named or chain id or the actual id value
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    /// Contains a known chain
    Named(ethers_core::types::Chain),
    /// Contains the id of a chain
    Id(u64),
}

impl Chain {
    /// The id of the chain
    pub fn id(&self) -> u64 {
        match self {
            Chain::Named(chain) => *chain as u64,
            Chain::Id(id) => *id,
        }
    }

    /// Helper function for checking if a chainid corresponds to a legacy chainid
    /// without eip1559
    pub fn is_legacy(&self) -> bool {
        match self {
            Chain::Named(c) => c.is_legacy(),
            Chain::Id(_) => false,
        }
    }
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Chain::Named(chain) => chain.fmt(f),
            Chain::Id(id) => {
                if let Ok(chain) = ethers_core::types::Chain::try_from(*id) {
                    chain.fmt(f)
                } else {
                    id.fmt(f)
                }
            }
        }
    }
}

impl From<ethers_core::types::Chain> for Chain {
    fn from(id: ethers_core::types::Chain) -> Self {
        Chain::Named(id)
    }
}

impl From<u64> for Chain {
    fn from(id: u64) -> Self {
        ethers_core::types::Chain::try_from(id).map(Chain::Named).unwrap_or_else(|_| Chain::Id(id))
    }
}

impl From<U256> for Chain {
    fn from(id: U256) -> Self {
        id.as_u64().into()
    }
}

impl From<Chain> for u64 {
    fn from(c: Chain) -> Self {
        match c {
            Chain::Named(c) => c as u64,
            Chain::Id(id) => id,
        }
    }
}

impl From<Chain> for U64 {
    fn from(c: Chain) -> Self {
        u64::from(c).into()
    }
}

impl From<Chain> for U256 {
    fn from(c: Chain) -> Self {
        u64::from(c).into()
    }
}

impl TryFrom<Chain> for ethers_core::types::Chain {
    type Error = ParseChainError;

    fn try_from(chain: Chain) -> Result<Self, Self::Error> {
        match chain {
            Chain::Named(chain) => Ok(chain),
            Chain::Id(id) => id.try_into(),
        }
    }
}

impl FromStr for Chain {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(chain) = ethers_core::types::Chain::from_str(s) {
            Ok(Chain::Named(chain))
        } else {
            s.parse::<u64>()
                .map(Chain::Id)
                .map_err(|_| format!("Expected known chain or integer, found: {s}"))
        }
    }
}

impl Encodable for Chain {
    fn length(&self) -> usize {
        match self {
            Self::Named(chain) => u64::from(*chain).length(),
            Self::Id(id) => id.length(),
        }
    }
    fn encode(&self, out: &mut dyn reth_rlp::BufMut) {
        match self {
            Self::Named(chain) => u64::from(*chain).encode(out),
            Self::Id(id) => id.encode(out),
        }
    }
}

impl Decodable for Chain {
    fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
        Ok(u64::decode(buf)?.into())
    }
}

impl Default for Chain {
    fn default() -> Self {
        ethers_core::types::Chain::Mainnet.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id() {
        let chain = Chain::Id(1234);
        assert_eq!(chain.id(), 1234);
    }

    #[test]
    fn test_named_id() {
        let chain = Chain::Named(ethers_core::types::Chain::Goerli);
        assert_eq!(chain.id(), 5);
    }

    #[test]
    fn test_legacy_named_chain() {
        let chain = Chain::Named(ethers_core::types::Chain::Optimism);
        assert!(chain.is_legacy());
    }

    #[test]
    fn test_not_legacy_named_chain() {
        let chain = Chain::Named(ethers_core::types::Chain::Mainnet);
        assert!(!chain.is_legacy());
    }

    #[test]
    fn test_not_legacy_id_chain() {
        let chain = Chain::Id(1234);
        assert!(!chain.is_legacy());
    }

    #[test]
    fn test_display_named_chain() {
        let chain = Chain::Named(ethers_core::types::Chain::Mainnet);
        assert_eq!(format!("{chain}"), "mainnet");
    }

    #[test]
    fn test_display_id_chain() {
        let chain = Chain::Id(1234);
        assert_eq!(format!("{chain}"), "1234");
    }

    #[test]
    fn test_from_u256() {
        let n = U256::from(1234);
        let chain = Chain::from(n);
        let expected = Chain::Id(1234);

        assert_eq!(chain, expected);
    }

    #[test]
    fn test_into_u256() {
        let chain = Chain::Named(ethers_core::types::Chain::Goerli);
        let n: U256 = chain.into();
        let expected = U256::from(5);

        assert_eq!(n, expected);
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_into_U64() {
        let chain = Chain::Named(ethers_core::types::Chain::Goerli);
        let n: U64 = chain.into();
        let expected = U64::from(5);

        assert_eq!(n, expected);
    }

    #[test]
    fn test_from_str_named_chain() {
        let result = Chain::from_str("mainnet");
        let expected = Chain::Named(ethers_core::types::Chain::Mainnet);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_from_str_named_chain_error() {
        let result = Chain::from_str("chain");

        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_id_chain() {
        let result = Chain::from_str("1234");
        let expected = Chain::Id(1234);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_default() {
        let default = Chain::default();
        let expected = Chain::Named(ethers_core::types::Chain::Mainnet);

        assert_eq!(default, expected);
    }

    #[test]
    fn test_id_chain_encodable_length() {
        let chain = Chain::Id(1234);

        assert_eq!(chain.length(), 3);
    }
}
