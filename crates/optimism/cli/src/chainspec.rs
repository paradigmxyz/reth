use std::{ffi::OsStr, sync::Arc};

use clap::{builder::TypedValueParser, error::Result, Arg, Command};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::args::utils::parse_custom_chain_spec;
use reth_optimism_chainspec::{
    OpChainSpec, BASE_MAINNET, BASE_SEPOLIA, OP_DEV, OP_MAINNET, OP_SEPOLIA,
};

/// Clap value parser for [`OpChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
fn chain_value_parser(s: &str) -> eyre::Result<Arc<OpChainSpec>, eyre::Error> {
    Ok(match s {
        "dev" => OP_DEV.clone(),
        "optimism" => OP_MAINNET.clone(),
        "optimism_sepolia" | "optimism-sepolia" => OP_SEPOLIA.clone(),
        "base" => BASE_MAINNET.clone(),
        "base_sepolia" | "base-sepolia" => BASE_SEPOLIA.clone(),
        _ => Arc::new(OpChainSpec { inner: parse_custom_chain_spec(s)? }),
    })
}

/// Optimism chain specification parser.
#[derive(Debug, Clone, Default)]
pub struct OpChainSpecParser;

impl ChainSpecParser<OpChainSpec> for OpChainSpecParser {
    const SUPPORTED_CHAINS: &'static [&'static str] = &[
        "dev",
        "optimism",
        "optimism_sepolia",
        "optimism-sepolia",
        "base",
        "base_sepolia",
        "base-sepolia",
    ];

    fn parse(s: &str) -> eyre::Result<Arc<OpChainSpec>> {
        chain_value_parser(s)
    }
}

impl TypedValueParser for OpChainSpecParser {
    type Value = Arc<OpChainSpec>;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        <Self as ChainSpecParser<OpChainSpec>>::parse(val).map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = Self::SUPPORTED_CHAINS.join(", ");
            clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                format!(
                    "Invalid value '{val}' for {arg}: {err}. [possible values: {possible_values}]"
                ),
            )
        })
    }

    fn possible_values(
        &self,
    ) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        let values = Self::SUPPORTED_CHAINS.iter().map(clap::builder::PossibleValue::new);
        Some(Box::new(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_chain_spec() {
        for &chain in OpChainSpecParser::SUPPORTED_CHAINS {
            assert!(<OpChainSpecParser as ChainSpecParser<OpChainSpec>>::parse(chain).is_ok());
        }
    }
}
