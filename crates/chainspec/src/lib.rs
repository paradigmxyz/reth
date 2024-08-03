//! The spec of an Ethereum network

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

pub use alloy_chains::{Chain, ChainKind, NamedChain};
pub use info::ChainInfo;
pub use spec::{
    BaseFeeParams, BaseFeeParamsKind, ChainSpec, ChainSpecBuilder, DepositContract,
    ForkBaseFeeParams, DEV, HOLESKY, MAINNET, SEPOLIA,
};
#[cfg(feature = "optimism")]
pub use spec::{BASE_MAINNET, BASE_SEPOLIA, OP_MAINNET, OP_SEPOLIA};

#[cfg(not(feature = "std"))]
extern crate alloc;

// /// The config info module namely spec id.
// pub mod config;
/// The chain info module.
mod info;

/// The chain spec module.
mod spec;

/// Chain specific constants
pub(crate) mod constants;

/// Re-export for convenience
pub use reth_ethereum_forks::*;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use alloy_rlp::Encodable;
    use std::str::FromStr;

    #[test]
    fn test_id() {
        let chain = Chain::from(1234);
        assert_eq!(chain.id(), 1234);
    }

    #[test]
    fn test_named_id() {
        let chain = Chain::from_named(NamedChain::Holesky);
        assert_eq!(chain.id(), 17000);
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
        let chain = Chain::from_named(NamedChain::Holesky);
        let n: U256 = U256::from(chain.id());
        let expected = U256::from(17000);

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
