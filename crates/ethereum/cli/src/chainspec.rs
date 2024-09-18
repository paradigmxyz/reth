use alloy_genesis::Genesis;
use reth_chainspec::{ChainSpec, DEV, HOLESKY, MAINNET, SEPOLIA};
use reth_cli::chainspec::ChainSpecParser;
use std::{fs, path::PathBuf, sync::Arc};

/// Clap value parser for [`ChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(match s {
        "mainnet" => MAINNET.clone(),
        "sepolia" => SEPOLIA.clone(),
        "holesky" => HOLESKY.clone(),
        "dev" => DEV.clone(),
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

/// Ethereum chain specification parser.
#[derive(Debug, Clone, Default)]
pub struct EthChainSpecParser;

impl ChainSpecParser for EthChainSpecParser {
    type ChainSpec = ChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &["mainnet", "sepolia", "holesky", "dev"];

    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>> {
        chain_value_parser(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::EthereumHardforks;

    #[test]
    fn parse_known_chain_spec() {
        for &chain in EthChainSpecParser::SUPPORTED_CHAINS {
            assert!(<EthChainSpecParser as ChainSpecParser>::parse(chain).is_ok());
        }
    }

    #[test]
    fn parse_raw_chainspec_hardforks() {
        let s = r#"{
  "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "uncleHash": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
  "coinbase": "0x0000000000000000000000000000000000000000",
  "stateRoot": "0x76f118cb05a8bc558388df9e3b4ad66ae1f17ef656e5308cb8f600717251b509",
  "transactionsTrie": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
  "receiptTrie": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
  "bloom": "0x000...000",
  "difficulty": "0x00",
  "number": "0x00",
  "gasLimit": "0x016345785d8a0000",
  "gasUsed": "0x00",
  "timestamp": "0x01",
  "extraData": "0x00",
  "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "nonce": "0x0000000000000000",
  "baseFeePerGas": "0x07",
  "withdrawalsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
  "blobGasUsed": "0x00",
  "excessBlobGas": "0x00",
  "parentBeaconBlockRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "requestsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
  "hash": "0xc20e1a771553139cdc77e6c3d5f64a7165d972d327eee9632c9c7d0fe839ded4",
  "alloc": {},
  "config": {
    "ethash": {},
    "chainId": 1,
    "homesteadBlock": 0,
    "daoForkSupport": true,
    "eip150Block": 0,
    "eip155Block": 0,
    "eip158Block": 0,
    "byzantiumBlock": 0,
    "constantinopleBlock": 0,
    "petersburgBlock": 0,
    "istanbulBlock": 0,
    "berlinBlock": 0,
    "londonBlock": 0,
    "terminalTotalDifficulty": 0,
    "shanghaiTime": 0,
    "cancunTime": 0,
    "pragueTime": 0
  }
}"#;

        let spec = <EthChainSpecParser as ChainSpecParser>::parse(s).unwrap();
        assert!(spec.hardforks.is_shanghai_active_at_timestamp(0));
        assert!(spec.hardforks.is_cancun_active_at_timestamp(0));
        assert!(spec.hardforks.is_prague_active_at_timestamp(0));
    }
}
