pub use alloy_chains::{Chain, NamedChain};
pub use info::ChainInfo;
pub use spec::{
    AllGenesisFormats, BaseFeeParams, BaseFeeParamsKind, ChainSpec, ChainSpecBuilder,
    DisplayHardforks, ForkBaseFeeParams, ForkCondition, ForkTimestamps, DEV, GOERLI, HOLESKY,
    MAINNET, SEPOLIA,
};
#[cfg(feature = "optimism")]
pub use spec::{BASE_GOERLI, BASE_MAINNET, BASE_SEPOLIA, OP_GOERLI};

// The chain spec module.
mod spec;
// The chain info module.
mod info;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::U256;
    use alloy_rlp::Encodable;
    use std::str::FromStr;

    #[test]
    fn test_id() {
        let chain = Chain::from(1234);
        assert_eq!(chain.id(), 1234);
    }

    #[test]
    fn test_named_id() {
        let chain = Chain::from_named(NamedChain::Goerli);
        assert_eq!(chain.id(), 5);
    }

    #[test]
    fn test_display_named_chain() {
        let chain = Chain::from_named(NamedChain::Mainnet);
        assert_eq!(format!("{chain}"), "mainnet");
    }

    #[test]
    fn test_display_id_chain() {
        let chain = Chain::from(1234);
        assert_eq!(format!("{chain}"), "1234");
    }

    #[test]
    fn test_from_u256() {
        let n = U256::from(1234);
        let chain = Chain::from(n.to::<u64>());
        let expected = Chain::from(1234);

        assert_eq!(chain, expected);
    }

    #[test]
    fn test_into_u256() {
        let chain = Chain::from_named(NamedChain::Goerli);
        let n: U256 = U256::from(chain.id());
        let expected = U256::from(5);

        assert_eq!(n, expected);
    }

    #[test]
    fn test_from_str_named_chain() {
        let result = Chain::from_str("mainnet");
        let expected = Chain::from_named(NamedChain::Mainnet);

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
        let expected = Chain::from(1234);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_default() {
        let default = Chain::default();
        let expected = Chain::from_named(NamedChain::Mainnet);

        assert_eq!(default, expected);
    }

    #[test]
    fn test_id_chain_encodable_length() {
        let chain = Chain::from(1234);

        assert_eq!(chain.length(), 3);
    }

    #[test]
    fn test_dns_main_network() {
        let s = "enrtree://AKA3AM6LPBYEUDMVNU3BSVQJ5AD45Y7YPOHJLEF6W26QOE4VTUDPE@all.mainnet.ethdisco.net";
        let chain: Chain = NamedChain::Mainnet.into();
        assert_eq!(s, chain.public_dns_network_protocol().unwrap().as_str());
    }

    #[test]
    fn test_dns_holesky_network() {
        let s = "enrtree://AKA3AM6LPBYEUDMVNU3BSVQJ5AD45Y7YPOHJLEF6W26QOE4VTUDPE@all.holesky.ethdisco.net";
        let chain: Chain = NamedChain::Holesky.into();
        assert_eq!(s, chain.public_dns_network_protocol().unwrap().as_str());
    }
}
