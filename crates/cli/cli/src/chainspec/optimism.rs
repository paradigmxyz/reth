use crate::chainspec::ChainSpecParser;
use alloy_genesis::Genesis;
use clap::{builder::TypedValueParser, error::Result, Arg, Command};
use reth_chainspec::{ChainSpec, BASE_MAINNET, BASE_SEPOLIA, OP_MAINNET, OP_SEPOLIA};
use std::{ffi::OsStr, fs, path::PathBuf, sync::Arc};

/// Optimism chain specification parser.
#[derive(Debug, Clone)]
pub struct OptimismChainSpecParser;

impl Default for OptimismChainSpecParser {
    fn default() -> Self {
        OptimismChainSpecParser
    }
}

impl ChainSpecParser for OptimismChainSpecParser {
    const SUPPORTED_CHAINS: &'static [&'static str] = &[
        "optimism",
        "optimism_sepolia",
        "optimism-sepolia",
        "base",
        "base_sepolia",
        "base-sepolia",
    ];

    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>> {
        Ok(match s {
            "optimism" => OP_MAINNET.clone(),
            "optimism_sepolia" | "optimism-sepolia" => OP_SEPOLIA.clone(),
            "base" => BASE_MAINNET.clone(),
            "base_sepolia" | "base-sepolia" => BASE_SEPOLIA.clone(),
            _ => {
                // try to read json from path first
                let raw =
                    match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
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
}

impl TypedValueParser for OptimismChainSpecParser {
    type Value = Arc<ChainSpec>;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        <OptimismChainSpecParser as ChainSpecParser>::parse(val).map_err(|err| {
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
    use crate::chainspec::{self, optimism};

    #[test]
    fn parse_known_chain_spec() {
        for &chain in OptimismChainSpecParser::SUPPORTED_CHAINS {
            assert!(<optimism::OptimismChainSpecParser as chainspec::ChainSpecParser>::parse(
                chain
            )
            .is_ok());
        }
    }
}
