//! Common helpers for reth-bench commands.

use crate::valid_payload::call_forkchoice_updated;
use eyre::Result;
use std::io::{BufReader, Read};

/// Read input from either a file path or stdin.
pub(crate) fn read_input(path: Option<&str>) -> Result<String> {
    Ok(match path {
        Some(path) => reth_fs_util::read_to_string(path)?,
        None => String::from_utf8(
            BufReader::new(std::io::stdin()).bytes().collect::<Result<Vec<_>, _>>()?,
        )?,
    })
}

/// Load JWT secret from either a file or use the provided string directly.
pub(crate) fn load_jwt_secret(jwt_secret: Option<&str>) -> Result<Option<String>> {
    match jwt_secret {
        Some(secret) => {
            // Try to read as file first
            match std::fs::read_to_string(secret) {
                Ok(contents) => Ok(Some(contents.trim().to_string())),
                // If file read fails, use the string directly
                Err(_) => Ok(Some(secret.to_string())),
            }
        }
        None => Ok(None),
    }
}

/// Parses a gas limit value with optional suffix: K for thousand, M for million, G for billion.
///
/// Examples: "30000000", "30M", "1G", "2G"
pub(crate) fn parse_gas_limit(s: &str) -> eyre::Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(eyre::eyre!("empty value"));
    }

    let (num_str, multiplier) = if let Some(prefix) = s.strip_suffix(['G', 'g']) {
        (prefix, 1_000_000_000u64)
    } else if let Some(prefix) = s.strip_suffix(['M', 'm']) {
        (prefix, 1_000_000u64)
    } else if let Some(prefix) = s.strip_suffix(['K', 'k']) {
        (prefix, 1_000u64)
    } else {
        (s, 1u64)
    };

    let base: u64 = num_str.trim().parse()?;
    base.checked_mul(multiplier).ok_or_else(|| eyre::eyre!("value overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gas_limit_plain_number() {
        assert_eq!(parse_gas_limit("30000000").unwrap(), 30_000_000);
        assert_eq!(parse_gas_limit("1").unwrap(), 1);
        assert_eq!(parse_gas_limit("0").unwrap(), 0);
    }

    #[test]
    fn test_parse_gas_limit_k_suffix() {
        assert_eq!(parse_gas_limit("1K").unwrap(), 1_000);
        assert_eq!(parse_gas_limit("30k").unwrap(), 30_000);
        assert_eq!(parse_gas_limit("100K").unwrap(), 100_000);
    }

    #[test]
    fn test_parse_gas_limit_m_suffix() {
        assert_eq!(parse_gas_limit("1M").unwrap(), 1_000_000);
        assert_eq!(parse_gas_limit("30m").unwrap(), 30_000_000);
        assert_eq!(parse_gas_limit("100M").unwrap(), 100_000_000);
    }

    #[test]
    fn test_parse_gas_limit_g_suffix() {
        assert_eq!(parse_gas_limit("1G").unwrap(), 1_000_000_000);
        assert_eq!(parse_gas_limit("2g").unwrap(), 2_000_000_000);
        assert_eq!(parse_gas_limit("10G").unwrap(), 10_000_000_000);
    }

    #[test]
    fn test_parse_gas_limit_with_whitespace() {
        assert_eq!(parse_gas_limit(" 1G ").unwrap(), 1_000_000_000);
        assert_eq!(parse_gas_limit("2 M").unwrap(), 2_000_000);
    }

    #[test]
    fn test_parse_gas_limit_errors() {
        assert!(parse_gas_limit("").is_err());
        assert!(parse_gas_limit("abc").is_err());
        assert!(parse_gas_limit("G").is_err());
        assert!(parse_gas_limit("-1G").is_err());
    }
}
