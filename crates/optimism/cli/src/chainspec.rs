use alloy_genesis::Genesis;
use clap::{builder::TypedValueParser, error::Result, Arg, Command};
use reth_chainspec::{ChainSpec, BASE_MAINNET, BASE_SEPOLIA, DEV, OP_MAINNET, OP_SEPOLIA};
use reth_cli::chainspec::ChainSpecParser;
use std::{ffi::OsStr, fs, path::PathBuf, sync::Arc};

/// Clap value parser for [`ChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(match s {
        "dev" => DEV.clone(),
        "optimism" => OP_MAINNET.clone(),
        "optimism_sepolia" | "optimism-sepolia" => OP_SEPOLIA.clone(),
        "base" => BASE_MAINNET.clone(),
        "base_sepolia" | "base-sepolia" => BASE_SEPOLIA.clone(),
        _ => {
            // try to read json from path first
            let raw = match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
                Ok(raw) => raw,
                Err(io_err) => {
                    // valid json may start with "\n", but must contain "{"
                    if s.contains('{') {
                        s.to_string()
                    } else {
                        return Err(io_err.into()) // assume invalid path
                    }
                }
            };

            // both serialized Genesis and ChainSpec structs supported
            let genesis: Genesis = serde_json::from_str(&raw)?;

            Arc::new(genesis.into())
        }
    })
}

/// Optimism chain specification parser.
#[derive(Debug, Clone, Default)]
pub struct OpChainSpecParser;

impl ChainSpecParser for OpChainSpecParser {
    const SUPPORTED_CHAINS: &'static [&'static str] = &[
        "optimism",
        "optimism_sepolia",
        "optimism-sepolia",
        "base",
        "base_sepolia",
        "base-sepolia",
    ];

    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>> {
        chain_value_parser(s)
    }
}

impl TypedValueParser for OpChainSpecParser {
    type Value = Arc<ChainSpec>;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        <Self as ChainSpecParser>::parse(val).map_err(|err| {
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
            assert!(<OpChainSpecParser as ChainSpecParser>::parse(chain).is_ok());
        }
    }
}
