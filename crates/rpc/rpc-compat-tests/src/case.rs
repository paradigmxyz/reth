//! Execution-apis `.io` fixture parsing and discovery.

use eyre::{eyre, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// One RPC compatibility test file.
#[derive(Debug, Clone)]
pub struct RpcTestCase {
    /// Canonical slash-separated path without the `.io` suffix.
    pub id: String,
    /// Human-readable header comments.
    pub description: String,
    /// Whether successful responses are validated against the specification schema.
    pub spec_only: bool,
    /// Ordered request/response exchanges.
    pub exchanges: Vec<RpcExchange>,
    /// Source file.
    pub source: PathBuf,
}

/// One request followed by its expected response.
#[derive(Debug, Clone)]
pub struct RpcExchange {
    /// Original request JSON.
    pub request_raw: String,
    /// Parsed request.
    pub request: Value,
    /// Original expected response JSON.
    pub expected_raw: String,
    /// Parsed expected response.
    pub expected: Value,
}

impl RpcTestCase {
    /// Parses one `.io` file.
    pub fn parse(id: String, source: PathBuf, contents: &str) -> Result<Self> {
        let mut description = String::new();
        let mut spec_only = false;
        let mut pending: Option<(String, Value)> = None;
        let mut exchanges = Vec::new();
        let mut in_header = true;

        for (index, untrimmed) in contents.lines().enumerate() {
            let line_number = index + 1;
            let line = untrimmed.trim();
            if line.is_empty() {
                continue
            }
            if let Some(comment) = line.strip_prefix("//") {
                if in_header {
                    let comment = comment.trim();
                    if comment.starts_with("speconly:") {
                        spec_only = true;
                    }
                    if !description.is_empty() {
                        description.push('\n');
                    }
                    description.push_str(comment);
                }
                continue
            }

            if let Some(raw) = line.strip_prefix(">>") {
                in_header = false;
                if pending.is_some() {
                    return Err(eyre!("{id}:{line_number}: request before previous response"))
                }
                let raw = raw.trim().to_string();
                let request = serde_json::from_str(&raw)
                    .wrap_err_with(|| format!("{id}:{line_number}: invalid request JSON"))?;
                pending = Some((raw, request));
                continue
            }

            if let Some(raw) = line.strip_prefix("<<") {
                in_header = false;
                let (request_raw, request) = pending
                    .take()
                    .ok_or_else(|| eyre!("{id}:{line_number}: response before request"))?;
                let raw = raw.trim().to_string();
                let expected = serde_json::from_str(&raw)
                    .wrap_err_with(|| format!("{id}:{line_number}: invalid response JSON"))?;
                exchanges.push(RpcExchange { request_raw, request, expected_raw: raw, expected });
                continue
            }

            return Err(eyre!("{id}:{line_number}: invalid fixture line {line:?}"))
        }

        if pending.is_some() {
            return Err(eyre!("{id}: request has no response"))
        }
        if exchanges.is_empty() {
            return Err(eyre!("{id}: fixture contains no request/response exchange"))
        }
        Ok(Self { id, description, spec_only, exchanges, source })
    }
}

/// Discovers and parses every `.io` file below the fixture directory.
pub fn discover(root: &Path) -> Result<Vec<RpcTestCase>> {
    let mut paths = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .wrap_err_with(|| format!("failed to walk fixture directory {}", root.display()))?
        .into_iter()
        .filter(|entry| {
            entry.file_type().is_file() &&
                entry.path().extension().and_then(|extension| extension.to_str()) == Some("io")
        })
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .wrap_err_with(|| format!("fixture path {} is outside root", path.display()))?;
            let mut id = relative.with_extension("").to_string_lossy().replace('\\', "/");
            while id.starts_with('/') {
                id.remove(0);
            }
            let contents = fs::read_to_string(&path)
                .wrap_err_with(|| format!("failed to read fixture {}", path.display()))?;
            RpcTestCase::parse(id, path, &contents)
        })
        .collect()
}

/// Loads alternative expected responses from either a `.io` fixture or JSON file.
pub fn load_response_variant(path: &Path) -> Result<Vec<Value>> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read response variant {}", path.display()))?;
    if path.extension().and_then(|extension| extension.to_str()) == Some("io") {
        let test =
            RpcTestCase::parse("response-variant".to_string(), path.to_path_buf(), &contents)?;
        Ok(test.exchanges.into_iter().map(|exchange| exchange.expected).collect())
    } else {
        let value = serde_json::from_str(&contents)
            .wrap_err_with(|| format!("invalid response variant JSON {}", path.display()))?;
        Ok(vec![value])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_exchanges_and_spec_only() {
        let fixture = r#"
// speconly: response depends on client configuration
>> {"jsonrpc":"2.0","id":1,"method":"eth_syncing"}
<< {"jsonrpc":"2.0","id":1,"result":false}
>> {"jsonrpc":"2.0","id":2,"method":"eth_chainId","params":[]}
<< {"jsonrpc":"2.0","id":2,"result":"0x1"}
"#;
        let test = RpcTestCase::parse("sample/test".to_string(), PathBuf::from("test.io"), fixture)
            .unwrap();
        assert!(test.spec_only);
        assert_eq!(test.exchanges.len(), 2);
    }
}
