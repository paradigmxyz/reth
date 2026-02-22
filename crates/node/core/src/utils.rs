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
use std::{
    env::VarError,
    path::{Path, PathBuf},
};
use tracing::{debug, info};

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, VarError> {
    let expanded = expand_path(value)?;
    Ok(PathBuf::from(expanded))
}

/// Expands `~` to the user's home directory and `$VAR`/`${VAR}` to environment variables.
fn expand_path(input: &str) -> Result<String, VarError> {
    // Expand tilde
    let input = if input == "~" {
        dirs_next::home_dir().map_or_else(|| input.to_string(), |h| h.display().to_string())
    } else if let Some(rest) = input.strip_prefix("~/") {
        dirs_next::home_dir()
            .map_or_else(|| input.to_string(), |h| format!("{}/{rest}", h.display()))
    } else {
        input.to_string()
    };

    // Expand environment variables ($VAR or ${VAR})
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next(); // consume '{'
            }
            let mut var_name = String::new();
            while let Some(&ch) = chars.peek() {
                if braced {
                    if ch == '}' {
                        chars.next();
                        break;
                    }
                } else if !ch.is_alphanumeric() && ch != '_' {
                    break;
                }
                var_name.push(ch);
                chars.next();
            }
            if var_name.is_empty() {
                result.push('$');
                if braced {
                    result.push('{');
                }
            } else {
                result.push_str(&std::env::var(&var_name)?);
            }
        } else {
            result.push(c);
        }
    }
    Ok(result)
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
