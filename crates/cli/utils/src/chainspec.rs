use reth_primitives::{ChainSpec, GOERLI, MAINNET, SEPOLIA};
use std::path::PathBuf;

/// Clap value parser for [ChainSpec]s that takes either a built-in chainspec or the path
/// to a custom one.
pub fn chain_spec_value_parser(s: &str) -> Result<ChainSpec, eyre::Error> {
    Ok(match s {
        "mainnet" => MAINNET.clone(),
        "goerli" => GOERLI.clone(),
        "sepolia" => SEPOLIA.clone(),
        _ => {
            let raw = std::fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned()))?;
            serde_json::from_str(&raw)?
        }
    })
}
