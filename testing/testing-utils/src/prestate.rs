//! Helpers for working with prestate dumps produced by geth's `prestateTracer`.

use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{hex, Address, Bytes, B256, U256};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::{self, Read},
    path::Path,
    str::FromStr,
};

/// Map of accounts that should be inserted into the genesis alloc.
pub type PrestateAlloc = BTreeMap<Address, GenesisAccount>;

/// Errors that can occur when parsing or applying a prestate dump.
#[derive(Debug)]
pub enum PrestateError {
    /// Failed to read the file.
    Io(io::Error),
    /// Failed to parse JSON.
    Json(serde_json::Error),
    /// A hex value (for code/storage fields) could not be decoded.
    Hex {
        /// Field name that failed to decode.
        field: &'static str,
        /// Offending hex value that caused the failure.
        value: String,
        /// Root decoding error.
        source: hex::FromHexError,
    },
    /// A quantity (balance) could not be represented as `U256`.
    InvalidQuantity {
        /// Field name associated with the invalid value.
        field: &'static str,
        /// Raw string that could not be parsed as a quantity.
        value: String,
    },
}

impl fmt::Display for PrestateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "failed to read prestate: {err}"),
            Self::Json(err) => write!(f, "failed to parse prestate json: {err}"),
            Self::Hex { field, value, .. } => {
                write!(f, "failed to decode hex for {field}: {value}")
            }
            Self::InvalidQuantity { field, value } => {
                write!(f, "invalid {field} quantity: {value}")
            }
        }
    }
}

impl std::error::Error for PrestateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Hex { source, .. } => Some(source),
            Self::InvalidQuantity { .. } => None,
        }
    }
}

impl From<io::Error> for PrestateError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for PrestateError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

/// Loads a geth prestate file and converts it into a [`PrestateAlloc`].
pub fn load_geth_prestate_from_file(
    path: impl AsRef<Path>,
) -> Result<PrestateAlloc, PrestateError> {
    let mut file = File::open(path)?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    parse_geth_prestate(&buf)
}

/// Parses a geth prestate (JSON) blob into a [`PrestateAlloc`].
pub fn parse_geth_prestate(data: &str) -> Result<PrestateAlloc, PrestateError> {
    let response: GethPrestateResponse = serde_json::from_str(data)?;
    response
        .result
        .into_iter()
        .map(|(address, account)| {
            let account = account.try_into()?;
            Ok((address, account))
        })
        .collect()
}

/// Extends the provided [`Genesis`] alloc with the parsed prestate accounts.
pub fn extend_genesis_with_prestate(genesis: &mut Genesis, alloc: PrestateAlloc) {
    genesis.alloc.extend(alloc);
}

#[derive(Debug, Deserialize)]
struct GethPrestateResponse {
    result: BTreeMap<Address, RawPrestateAccount>,
}

#[derive(Debug, Deserialize)]
struct RawPrestateAccount {
    #[serde(default = "zero_hex")]
    balance: String,
    #[serde(default)]
    nonce: Option<u64>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    storage: BTreeMap<String, String>,
    #[serde(default, rename = "codeHash")]
    _code_hash: Option<String>,
}

impl TryFrom<RawPrestateAccount> for GenesisAccount {
    type Error = PrestateError;

    fn try_from(value: RawPrestateAccount) -> Result<Self, Self::Error> {
        let balance = parse_u256("balance", &value.balance)?;
        let code = match value.code {
            Some(code) if code.is_empty() || code == "0x" => None,
            Some(code) => Some(Bytes::from_str(&code).map_err(|source| PrestateError::Hex {
                field: "code",
                value: code,
                source,
            })?),
            None => None,
        };

        let storage = if value.storage.is_empty() {
            None
        } else {
            let mut entries = BTreeMap::new();
            for (slot, val) in value.storage {
                let key = B256::from_str(&slot).map_err(|source| PrestateError::Hex {
                    field: "storage key",
                    value: slot,
                    source,
                })?;
                let value = B256::from_str(&val).map_err(|source| PrestateError::Hex {
                    field: "storage value",
                    value: val,
                    source,
                })?;
                entries.insert(key, value);
            }
            Some(entries)
        };

        Ok(Self::default()
            .with_balance(balance)
            .with_nonce(value.nonce)
            .with_code(code)
            .with_storage(storage))
    }
}

fn parse_u256(field: &'static str, value: &str) -> Result<U256, PrestateError> {
    let trimmed = value.trim();
    if let Some(stripped) = trimmed.strip_prefix("0x") {
        if stripped.is_empty() {
            return Ok(U256::ZERO)
        }

        let normalized = if stripped.len() % 2 == 1 {
            let mut prefixed = String::with_capacity(stripped.len() + 1);
            prefixed.push('0');
            prefixed.push_str(stripped);
            prefixed
        } else {
            stripped.to_owned()
        };

        let decoded = hex::decode(normalized.as_str()).map_err(|source| PrestateError::Hex {
            field,
            value: value.to_string(),
            source,
        })?;
        if decoded.len() > 32 {
            return Err(PrestateError::InvalidQuantity { field, value: value.to_string() })
        }
        let mut buf = [0u8; 32];
        buf[32 - decoded.len()..].copy_from_slice(&decoded);
        Ok(U256::from_be_bytes(buf))
    } else {
        trimmed
            .parse::<U256>()
            .map_err(|_| PrestateError::InvalidQuantity { field, value: value.to_string() })
    }
}

fn zero_hex() -> String {
    "0x0".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
