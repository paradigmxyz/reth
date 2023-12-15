use crate::{
    holesky_nodes,
    net::{goerli_nodes, mainnet_nodes, sepolia_nodes},
    NodeRecord, U256, U64,
};
use alloy_rlp::{Decodable, Encodable};
use num_enum::TryFromPrimitive;
use reth_codecs::add_arbitrary_tests;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use strum::{AsRefStr, EnumCount, EnumIter, EnumString, EnumVariantNames};

// The chain spec module.
mod spec;
pub use spec::{
    AllGenesisFormats, BaseFeeParams, BaseFeeParamsKind, ChainSpec, ChainSpecBuilder,
    DisplayHardforks, ForkBaseFeeParams, ForkCondition, ForkTimestamps, DEV, GOERLI, HOLESKY,
    MAINNET, SEPOLIA,
};

#[cfg(feature = "optimism")]
pub use spec::{BASE_GOERLI, BASE_MAINNET, OP_GOERLI};

// The chain info module.
mod info;
pub use info::ChainInfo;

/// An Ethereum EIP-155 chain.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    AsRefStr,         // AsRef<str>, fmt::Display
    EnumVariantNames, // Chain::VARIANTS
    EnumString,       // FromStr, TryFrom<&str>
    EnumIter,         // Chain::iter
    EnumCount,        // Chain::COUNT
    TryFromPrimitive, // TryFrom<u64>
    Deserialize,
    Serialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u64)]
#[allow(missing_docs)]
pub enum NamedChain {
    Mainnet = 1,
    Morden = 2,
    Ropsten = 3,
    Rinkeby = 4,
    Goerli = 5,
    Kovan = 42,
    Holesky = 17000,
    Sepolia = 11155111,

    Optimism = 10,
    OptimismKovan = 69,
    OptimismGoerli = 420,

    Base = 8453,
    BaseGoerli = 84531,

    Arbitrum = 42161,
    ArbitrumTestnet = 421611,
    ArbitrumGoerli = 421613,
    ArbitrumNova = 42170,

    #[serde(alias = "bsc")]
    #[strum(to_string = "bsc")]
    BinanceSmartChain = 56,
    #[serde(alias = "bsc_testnet")]
    #[strum(to_string = "bsc_testnet")]
    BinanceSmartChainTestnet = 97,

    Dev = 1337,
}

impl From<NamedChain> for u64 {
    fn from(value: NamedChain) -> Self {
        value as u64
    }
}

impl fmt::Display for NamedChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

/// Either a named or chain id or the actual id value
#[add_arbitrary_tests(rlp)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    /// Contains a known chain
    Named(NamedChain),
    /// Contains the id of a chain
    Id(u64),
}

impl Chain {
    /// Returns the mainnet chain.
    pub const fn mainnet() -> Self {
        Chain::Named(NamedChain::Mainnet)
    }

    /// Returns the goerli chain.
    pub const fn goerli() -> Self {
        Chain::Named(NamedChain::Goerli)
    }

    /// Returns the sepolia chain.
    pub const fn sepolia() -> Self {
        Chain::Named(NamedChain::Sepolia)
    }

    /// Returns the holesky chain.
    pub const fn holesky() -> Self {
        Chain::Named(NamedChain::Holesky)
    }

    /// Returns the optimism goerli chain.
    pub const fn optimism_goerli() -> Self {
        Chain::Named(NamedChain::OptimismGoerli)
    }

    /// Returns the optimism mainnet chain.
    pub const fn optimism_mainnet() -> Self {
        Chain::Named(NamedChain::Optimism)
    }

    /// Returns the base goerli chain.
    pub const fn base_goerli() -> Self {
        Chain::Named(NamedChain::BaseGoerli)
    }

    /// Returns the base mainnet chain.
    pub const fn base_mainnet() -> Self {
        Chain::Named(NamedChain::Base)
    }

    /// Returns the dev chain.
    pub const fn dev() -> Self {
        Chain::Named(NamedChain::Dev)
    }

    /// Returns true if the chain contains Optimism configuration.
    pub fn is_optimism(self) -> bool {
        self.named().map_or(false, |c| {
            matches!(
                c,
                NamedChain::Optimism |
                    NamedChain::OptimismGoerli |
                    NamedChain::OptimismKovan |
                    NamedChain::Base |
                    NamedChain::BaseGoerli
            )
        })
    }

    /// Attempts to convert the chain into a named chain.
    pub fn named(&self) -> Option<NamedChain> {
        match self {
            Chain::Named(chain) => Some(*chain),
            Chain::Id(id) => NamedChain::try_from(*id).ok(),
        }
    }

    /// The id of the chain
    pub fn id(&self) -> u64 {
        match self {
            Chain::Named(chain) => *chain as u64,
            Chain::Id(id) => *id,
        }
    }

    /// Returns the address of the public DNS node list for the given chain.
    ///
    /// See also <https://github.com/ethereum/discv4-dns-lists>
    pub fn public_dns_network_protocol(self) -> Option<String> {
        use NamedChain as C;
        const DNS_PREFIX: &str = "enrtree://AKA3AM6LPBYEUDMVNU3BSVQJ5AD45Y7YPOHJLEF6W26QOE4VTUDPE@";

        let named: NamedChain = self.try_into().ok()?;

        if matches!(named, C::Mainnet | C::Goerli | C::Sepolia | C::Ropsten | C::Rinkeby) {
            return Some(format!("{DNS_PREFIX}all.{}.ethdisco.net", named.as_ref().to_lowercase()))
        }
        None
    }

    /// Returns bootnodes for the given chain.
    pub fn bootnodes(self) -> Option<Vec<NodeRecord>> {
        use NamedChain as C;
        match self.try_into().ok()? {
            C::Mainnet => Some(mainnet_nodes()),
            C::Goerli => Some(goerli_nodes()),
            C::Sepolia => Some(sepolia_nodes()),
            C::Holesky => Some(holesky_nodes()),
            _ => None,
        }
    }
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Chain::Named(chain) => chain.fmt(f),
            Chain::Id(id) => {
                if let Ok(chain) = NamedChain::try_from(*id) {
                    chain.fmt(f)
                } else {
                    id.fmt(f)
                }
            }
        }
    }
}

impl From<NamedChain> for Chain {
    fn from(id: NamedChain) -> Self {
        Chain::Named(id)
    }
}

impl From<u64> for Chain {
    fn from(id: u64) -> Self {
        NamedChain::try_from(id).map(Chain::Named).unwrap_or_else(|_| Chain::Id(id))
    }
}

impl From<U256> for Chain {
    fn from(id: U256) -> Self {
        id.to::<u64>().into()
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
        U64::from(u64::from(c))
    }
}

impl From<Chain> for U256 {
    fn from(c: Chain) -> Self {
        U256::from(u64::from(c))
    }
}

impl TryFrom<Chain> for NamedChain {
    type Error = <NamedChain as TryFrom<u64>>::Error;

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
        if let Ok(chain) = NamedChain::from_str(s) {
            Ok(Chain::Named(chain))
        } else {
            s.parse::<u64>()
                .map(Chain::Id)
                .map_err(|_| format!("Expected known chain or integer, found: {s}"))
        }
    }
}

impl Encodable for Chain {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::Named(chain) => u64::from(*chain).encode(out),
            Self::Id(id) => id.encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            Self::Named(chain) => u64::from(*chain).length(),
            Self::Id(id) => id.length(),
        }
    }
}

impl Decodable for Chain {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(u64::decode(buf)?.into())
    }
}

impl Default for Chain {
    fn default() -> Self {
        NamedChain::Mainnet.into()
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Chain {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        if u.ratio(1, 2)? {
            let chain = u.int_in_range(0..=(NamedChain::COUNT - 1))?;

            return Ok(Chain::Named(NamedChain::iter().nth(chain).expect("in range")))
        }

        Ok(Self::Id(u64::arbitrary(u)?))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
use strum::IntoEnumIterator;

#[cfg(any(test, feature = "arbitrary"))]
use proptest::{
    arbitrary::ParamsFor,
    prelude::{any, Strategy},
    sample::Selector,
    strategy::BoxedStrategy,
};

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for Chain {
    type Parameters = ParamsFor<u32>;
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        let named =
            any::<Selector>().prop_map(move |sel| Chain::Named(sel.select(NamedChain::iter())));
        let id = any::<u64>().prop_map(Chain::from);
        proptest::strategy::Union::new_weighted(vec![(50, named.boxed()), (50, id.boxed())]).boxed()
    }

    type Strategy = BoxedStrategy<Chain>;
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
        let chain = Chain::Named(NamedChain::Goerli);
        assert_eq!(chain.id(), 5);
    }

    #[test]
    fn test_display_named_chain() {
        let chain = Chain::Named(NamedChain::Mainnet);
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
        let chain = Chain::Named(NamedChain::Goerli);
        let n: U256 = chain.into();
        let expected = U256::from(5);

        assert_eq!(n, expected);
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_into_U64() {
        let chain = Chain::Named(NamedChain::Goerli);
        let n: U64 = chain.into();
        let expected = U64::from(5);

        assert_eq!(n, expected);
    }

    #[test]
    fn test_from_str_named_chain() {
        let result = Chain::from_str("mainnet");
        let expected = Chain::Named(NamedChain::Mainnet);

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
        let expected = Chain::Named(NamedChain::Mainnet);

        assert_eq!(default, expected);
    }

    #[test]
    fn test_id_chain_encodable_length() {
        let chain = Chain::Id(1234);

        assert_eq!(chain.length(), 3);
    }

    #[test]
    fn test_dns_network() {
        let s = "enrtree://AKA3AM6LPBYEUDMVNU3BSVQJ5AD45Y7YPOHJLEF6W26QOE4VTUDPE@all.mainnet.ethdisco.net";
        let chain: Chain = NamedChain::Mainnet.into();
        assert_eq!(s, chain.public_dns_network_protocol().unwrap().as_str());
    }
}
