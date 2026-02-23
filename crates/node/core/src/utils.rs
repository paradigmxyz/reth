//! Utility functions for node startup and shutdown, for example path parsing and retrieving single
//! blocks from the network.

use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_rpc_types_engine::{JwtError, JwtSecret};
use eyre::Result;
use reth_consensus::Consensus;
use reth_network_p2p::{
    bodies::client::BodiesClient, headers::client::HeadersClient, priority::Priority,
};
use reth_primitives_traits::{Block, SealedBlock, SealedHeader};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, ExpandPathError> {
    let expanded = expand_path(value)?;
    Ok(PathBuf::from(expanded))
}

/// Expands `~` to the user's home directory and `$VAR`/`${VAR}` to environment variable values.
fn expand_path(input: &str) -> Result<String, ExpandPathError> {
    let tilde_expanded = expand_tilde(input)?;
    expand_env_vars(&tilde_expanded)
}

fn expand_tilde(input: &str) -> Result<String, ExpandPathError> {
    if input == "~" || input.starts_with("~/") || input.starts_with("~\\") {
        let home = dirs_next::home_dir().ok_or(ExpandPathError::NoHomeDir)?;
        let mut out = home.to_string_lossy().into_owned();
        if input.len() > 1 {
            out.push_str(&input[1..]);
        }
        Ok(out)
    } else {
        Ok(input.to_string())
    }
}

fn expand_env_vars(input: &str) -> Result<String, ExpandPathError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '$' {
            result.push(c);
            continue;
        }

        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }

        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if braced {
                if c == '}' {
                    chars.next();
                    break;
                }
            } else if !c.is_ascii_alphanumeric() && c != '_' {
                break;
            }
            name.push(c);
            chars.next();
        }

        if name.is_empty() {
            result.push('$');
            if braced {
                result.push('{');
            }
        } else {
            let value = std::env::var(&name)
                .map_err(|e| ExpandPathError::Var { var_name: name, source: e })?;
            result.push_str(&value);
        }
    }

    Ok(result)
}

/// An error that can occur when expanding a path.
#[derive(Debug, thiserror::Error)]
pub enum ExpandPathError {
    /// Home directory could not be determined.
    #[error("could not determine home directory")]
    NoHomeDir,
    /// Environment variable lookup failed.
    #[error("environment variable `{var_name}` not found: {source}")]
    Var {
        /// The variable name that was looked up.
        var_name: String,
        /// The underlying error.
        source: std::env::VarError,
    },
}

/// Attempts to retrieve or create a JWT secret from the specified path.
pub fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
    if path.exists() {
        debug!(target: "reth::cli", ?path, "Reading JWT auth secret file");
        JwtSecret::from_file(path)
    } else {
        info!(target: "reth::cli", ?path, "Creating JWT auth secret file");
        JwtSecret::try_create_random(path)
    }
}

/// Get a single header from the network
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> Result<SealedHeader<Client::Header>>
where
    Client: HeadersClient<Header: reth_primitives_traits::BlockHeader>,
{
    let (peer_id, response) = client.get_header_with_priority(id, Priority::High).await?.split();

    let Some(header) = response else {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: 0")
    };

    let header = SealedHeader::seal_slow(header);

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number() == number,
    };

    if !valid {
        client.report_bad_message(peer_id);
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}

/// Get a body from the network based on header
pub async fn get_single_body<B, Client>(
    client: Client,
    header: SealedHeader<B::Header>,
    consensus: impl Consensus<B>,
) -> Result<SealedBlock<B>>
where
    B: Block,
    Client: BodiesClient<Body = B::Body>,
{
    let (peer_id, response) = client.get_block_body(header.hash()).await?.split();

    let Some(body) = response else {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of bodies received. Expected: 1. Received: 0")
    };

    let block = SealedBlock::from_sealed_parts(header, body);
    consensus.validate_block_pre_execution(&block)?;

    Ok(block)
}
