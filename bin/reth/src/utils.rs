//! Common CLI utility functions.

use eyre::{Result, WrapErr};
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::p2p::{
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{
    AllGenesisFormats, BlockHashOrNumber, ChainSpec, HeadersDirection, SealedHeader, GOERLI, H256,
    MAINNET, SEPOLIA,
};
use std::{
    env::VarError,
    net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tracing::info;

/// Get a single header from network
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> Result<SealedHeader>
where
    Client: HeadersClient,
{
    let request = HeadersRequest { direction: HeadersDirection::Rising, limit: 1, start: id };

    let (peer_id, response) =
        client.get_headers_with_priority(request, Priority::High).await?.split();

    if response.len() != 1 {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: {}", response.len())
    }

    let header = response.into_iter().next().unwrap().seal_slow();

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number == number,
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

/// Wrapper over DB that implements many useful DB queries.
pub struct DbTool<'a, DB: Database> {
    pub(crate) db: &'a DB,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created.
    pub(crate) fn new(db: &'a DB) -> eyre::Result<Self> {
        Ok(Self { db })
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [`HashMap`][std::collections::HashMap].
    pub fn list<T: Table>(
        &mut self,
        skip: usize,
        len: usize,
        reverse: bool,
    ) -> Result<Vec<(T::Key, T::Value)>> {
        let data = self.db.view(|tx| {
            let mut cursor = tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

            if reverse {
                cursor.walk_back(None)?.skip(skip).take(len).collect::<Result<_, _>>()
            } else {
                cursor.walk(None)?.skip(skip).take(len).collect::<Result<_, _>>()
            }
        })?;

        data.map_err(|e| eyre::eyre!(e))
    }

    /// Grabs the content of the table for the given key
    pub fn get<T: Table>(&mut self, key: T::Key) -> Result<Option<T::Value>> {
        self.db.view(|tx| tx.get::<T>(key))?.map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database at the given path.
    pub fn drop(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        info!(target: "reth::cli", "Dropping db at {:?}", path);
        std::fs::remove_dir_all(path).wrap_err("Dropping the database failed")?;
        Ok(())
    }

    /// Drops the provided table from the database.
    pub fn drop_table<T: Table>(&mut self) -> Result<()> {
        self.db.update(|tx| tx.clear::<T>())??;
        Ok(())
    }
}

/// Helper to parse a [Duration] from seconds
pub fn parse_duration_from_secs(arg: &str) -> Result<Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(Duration::from_secs(seconds))
}

/// Clap value parser for [ChainSpec]s that takes either a built-in chainspec or the path
/// to a custom one.
pub fn chain_spec_value_parser(s: &str) -> Result<Arc<ChainSpec>, eyre::Error> {
    Ok(Arc::new(match s {
        "mainnet" => MAINNET.clone(),
        "goerli" => GOERLI.clone(),
        "sepolia" => SEPOLIA.clone(),
        _ => {
            let raw = std::fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned()))?;
            serde_json::from_str(&raw)?
        }
    }))
}

/// Clap value parser for [ChainSpec]s that takes either a built-in genesis format or the path
/// to a custom one.
pub fn genesis_value_parser(s: &str) -> Result<Arc<ChainSpec>, eyre::Error> {
    Ok(Arc::new(match s {
        "mainnet" => MAINNET.clone(),
        "goerli" => GOERLI.clone(),
        "sepolia" => SEPOLIA.clone(),
        _ => {
            let raw = std::fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned()))?;
            let genesis: AllGenesisFormats = serde_json::from_str(&raw)?;
            genesis.into()
        }
    }))
}

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, shellexpand::LookupError<VarError>> {
    shellexpand::full(value).map(|path| PathBuf::from(path.into_owned()))
}

/// Parse [BlockHashOrNumber]
pub fn hash_or_num_value_parser(value: &str) -> Result<BlockHashOrNumber, eyre::Error> {
    match H256::from_str(value) {
        Ok(hash) => Ok(BlockHashOrNumber::Hash(hash)),
        Err(_) => Ok(BlockHashOrNumber::Number(value.parse()?)),
    }
}

/// Error thrown while parsing a socket address.
#[derive(thiserror::Error, Debug)]
pub enum SocketAddressParsingError {
    /// Failed to convert the string into a socket addr
    #[error("Cannot parse socket address: {0}")]
    Io(#[from] std::io::Error),
    /// Input must not be empty
    #[error("Cannot parse socket address from empty string")]
    Empty,
    /// Failed to parse the address
    #[error("Could not parse socket address from {0}")]
    Parse(String),
    /// Failed to parse port
    #[error("Could not parse port: {0}")]
    Port(#[from] std::num::ParseIntError),
}

/// Parse a [SocketAddr] from a `str`.
///
/// The following formats are checked:
///
/// - If the value can be parsed as a `u16` or starts with `:` it is considered a port, and the
/// hostname is set to `localhost`.
/// - If the value contains `:` it is assumed to be the format `<host>:<port>`
/// - Otherwise it is assumed to be a hostname
///
/// An error is returned if the value is empty.
pub fn parse_socket_address(value: &str) -> Result<SocketAddr, SocketAddressParsingError> {
    if value.is_empty() {
        return Err(SocketAddressParsingError::Empty)
    }

    if let Some(port) = value.strip_prefix(':').or_else(|| value.strip_prefix("localhost:")) {
        let port: u16 = port.parse()?;
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
    }
    if let Ok(port) = value.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
    }
    value
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| SocketAddressParsingError::Parse(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chain_spec() {
        for chain in ["mainnet", "sepolia", "goerli"] {
            chain_spec_value_parser(chain).unwrap();
            genesis_value_parser(chain).unwrap();
        }
    }

    #[test]
    fn parse_socket_addresses() {
        for value in ["localhost:9000", ":9000", "9000"] {
            let socket_addr = parse_socket_address(value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), 9000);
        }
    }

    #[test]
    fn parse_socket_address_random() {
        let port: u16 = thread_rng().gen();

        for value in [format!("localhost:{port}"), format!(":{port}"), port.to_string()] {
            let socket_addr = parse_socket_address(&value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), port);
        }
    }
}
