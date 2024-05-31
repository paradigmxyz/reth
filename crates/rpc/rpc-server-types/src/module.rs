use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize, Serializer};
use strum::{AsRefStr, EnumIter, IntoStaticStr, ParseError, VariantArray, VariantNames};

/// Represents RPC modules that are supported by reth
#[derive(
    Debug,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Hash,
    AsRefStr,
    IntoStaticStr,
    VariantNames,
    VariantArray,
    EnumIter,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "kebab-case")]
pub enum RethRpcModule {
    /// `admin_` module
    Admin,
    /// `debug_` module
    Debug,
    /// `eth_` module
    Eth,
    /// `net_` module
    Net,
    /// `trace_` module
    Trace,
    /// `txpool_` module
    Txpool,
    /// `web3_` module
    Web3,
    /// `rpc_` module
    Rpc,
    /// `reth_` module
    Reth,
    /// `ots_` module
    Ots,
    /// For single non-standard `eth_` namespace call `eth_callBundle`
    ///
    /// This is separate from [RethRpcModule::Eth] because it is a non standardized call that
    /// should be opt-in.
    EthCallBundle,
}

// === impl RethRpcModule ===

impl RethRpcModule {
    /// Returns the number of variants in the enum
    pub const fn variant_count() -> usize {
        <Self as VariantArray>::VARIANTS.len()
    }

    /// Returns all variant names of the enum
    pub const fn all_variant_names() -> &'static [&'static str] {
        <Self as VariantNames>::VARIANTS
    }

    /// Returns all variants of the enum
    pub const fn all_variants() -> &'static [Self] {
        <Self as VariantArray>::VARIANTS
    }

    /// Returns all variants of the enum
    pub fn modules() -> impl IntoIterator<Item = Self> {
        use strum::IntoEnumIterator;
        Self::iter()
    }

    /// Returns the string representation of the module.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

impl FromStr for RethRpcModule {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "admin" => Self::Admin,
            "debug" => Self::Debug,
            "eth" => Self::Eth,
            "net" => Self::Net,
            "trace" => Self::Trace,
            "txpool" => Self::Txpool,
            "web3" => Self::Web3,
            "rpc" => Self::Rpc,
            "reth" => Self::Reth,
            "ots" => Self::Ots,
            "eth-call-bundle" | "eth_callBundle" => Self::EthCallBundle,
            _ => return Err(ParseError::VariantNotFound),
        })
    }
}

impl TryFrom<&str> for RethRpcModule {
    type Error = ParseError;
    fn try_from(s: &str) -> Result<Self, <Self as TryFrom<&str>>::Error> {
        FromStr::from_str(s)
    }
}

impl fmt::Display for RethRpcModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_ref())
    }
}

impl Serialize for RethRpcModule {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(self.as_ref())
    }
}
