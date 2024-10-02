use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_optimism_chainspec::{
    OpChainSpec, BASE_MAINNET, BASE_SEPOLIA, OP_DEV, OP_MAINNET, OP_SEPOLIA,
};
use std::{path::PathBuf, sync::Arc};

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

/// Parses a custom [`ChainSpec`].
pub fn parse_custom_chain_spec(s: &str) -> eyre::Result<ChainSpec, eyre::Error> {
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

    Ok(genesis.into())
}

/// Optimism chain specification parser.
#[derive(Debug, Clone, Default)]
pub struct OpChainSpecParser;

impl ChainSpecParser for OpChainSpecParser {
    type ChainSpec = OpChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &[
        "dev",
        "optimism",
        "optimism_sepolia",
        "optimism-sepolia",
        "base",
        "base_sepolia",
        "base-sepolia",
    ];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        chain_value_parser(s)
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
