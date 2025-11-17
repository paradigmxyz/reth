//! Helpers for working with prestate dumps produced by geth's `prestateTracer`.

use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, Bytes};
use alloy_rpc_types_trace::geth::{AccountState, PreStateFrame};
use eyre::{Context as _, Result};
use serde::Deserialize;
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

/// Map of accounts that should be inserted into the genesis alloc.
pub type PrestateAlloc = BTreeMap<Address, GenesisAccount>;

/// Loads a geth prestate file and converts it into a [`PrestateAlloc`].
pub fn load_geth_prestate_from_file(path: impl AsRef<Path>) -> Result<PrestateAlloc> {
    let mut file = File::open(path.as_ref()).wrap_err("failed to open prestate file")?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).wrap_err("failed to read prestate file")?;
    parse_geth_prestate(&buf)
}

/// Parses a geth prestate (JSON) blob into a [`PrestateAlloc`].
pub fn parse_geth_prestate(data: &str) -> Result<PrestateAlloc> {
    let response: GethPrestateResponse =
        serde_json::from_str(data).wrap_err("failed to parse prestate JSON")?;

    let accounts = match response.result {
        PreStateFrame::Default(mode) => mode.0,
        PreStateFrame::Diff(diff) => diff.pre,
    };

    Ok(accounts
        .into_iter()
        .map(|(addr, state)| (addr, account_state_to_genesis(state)))
        .collect::<PrestateAlloc>())
}

/// Extends the provided [`Genesis`] alloc with the parsed prestate accounts.
pub fn extend_genesis_with_prestate(genesis: &mut Genesis, alloc: PrestateAlloc) {
    genesis.alloc.extend(alloc);
}

#[derive(Debug, Deserialize)]
struct GethPrestateResponse {
    result: PreStateFrame,
}

fn account_state_to_genesis(value: AccountState) -> GenesisAccount {
    let balance = value.balance.unwrap_or_default();
    let code = value.code.filter(|code: &Bytes| !code.is_empty());
    let storage = (!value.storage.is_empty()).then_some(value.storage);

    GenesisAccount::default()
        .with_balance(balance)
        .with_nonce(value.nonce)
        .with_code(code)
        .with_storage(storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn parses_simple_prestate() {
        let json = r#"
        {
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "0x0000000000000000000000000000000000000001": {
                    "balance": "0x1",
                    "nonce": 1,
                    "code": "0x6000",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000001"
                    }
                }
            }
        }"#;

        let alloc = parse_geth_prestate(json).unwrap();
        assert_eq!(alloc.len(), 1);
        let account = alloc.values().next().unwrap();
        assert_eq!(account.balance, U256::from(1u64));
        assert_eq!(account.nonce, Some(1));
        assert!(account.code.as_ref().is_some());
        assert!(account.storage.as_ref().is_some());
    }
}
