//! Root
//! Includes common helpers for integration tests
//!
//! These tests use the `reth-era-downloader` client to download `.era1` files temporarily
//! and verify that we can correctly read and decompress their data.
//!
//! Files are downloaded from [`MAINNET_URL`] and [`SEPOLIA_URL`].

use reqwest::{Client, Url};
use reth_era::{
    e2s_types::E2sError,
    era1_file::{Era1File, Era1Reader},
};
use reth_era_downloader::EraClient;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use eyre::{eyre, Result};
use tempfile::TempDir;

mod dd;
mod genesis;
mod roundtrip;

const fn main() {}

/// Mainnet network name
const MAINNET: &str = "mainnet";
/// Default mainnet url
/// for downloading mainnet `.era1` files
const MAINNET_URL: &str = "https://era.ithaca.xyz/era1/index.html";

/// Succinct list of mainnet files we want to download
/// from <https://era.ithaca.xyz/era1/>
/// for testing purposes
const ERA1_MAINNET_FILES_NAMES: [&str; 6] = [
    "mainnet-00000-5ec1ffb8.era1",
    "mainnet-00003-d8b8a40b.era1",
    "mainnet-00151-e322efe1.era1",
    "mainnet-00293-0d6c5812.era1",
    "mainnet-00443-ea71b6f9.era1",
    "mainnet-01367-d7efc68f.era1",
];

/// Sepolia network name
const SEPOLIA: &str = "sepolia";

/// Default sepolia mainnet url
/// for downloading sepolia `.era1` files
const SEPOLIA_URL: &str = "https://era.ithaca.xyz/sepolia-era1/";

/// Succinct list of sepolia files we want to download
/// from <https://era.ithaca.xyz/sepolia-era1/>
/// for testing purposes
const ERA1_SEPOLIA_FILES_NAMES: [&str; 3] =
    ["sepolia-00000-643a00f7.era1", "sepolia-00074-0e81003c.era1", "sepolia-00173-b6924da5.era1"];

/// Utility for downloading `.era1` files for tests
/// in a temporary directory
/// and caching them in memory
#[derive(Debug)]
struct Era1TestDownloader {
    /// Temporary directory for storing downloaded files
    temp_dir: TempDir,
    /// Cache mapping file names to their paths
    file_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl Era1TestDownloader {
    /// Create a new downloader instance with a temporary directory
    async fn new() -> Result<Self> {
        let temp_dir =
            TempDir::new().map_err(|e| eyre!("Failed to create temp directory: {}", e))?;

        Ok(Self { temp_dir, file_cache: Arc::new(Mutex::new(HashMap::new())) })
    }

    /// Download a specific .era1 file by name
    pub(crate) async fn download_file(&self, filename: &str, network: &str) -> Result<PathBuf> {
        // check cache first
        {
            let cache = self.file_cache.lock().unwrap();
            if let Some(path) = cache.get(filename) {
                return Ok(path.clone());
            }
        }

        // check if the filename is supported
        if !ERA1_MAINNET_FILES_NAMES.contains(&filename) &&
            !ERA1_SEPOLIA_FILES_NAMES.contains(&filename)
        {
            return Err(eyre!(
                "Unknown file: {}. Only the following files are supported: {:?} or {:?}",
                filename,
                ERA1_MAINNET_FILES_NAMES,
                ERA1_SEPOLIA_FILES_NAMES
            ));
        }

        // initialize the client and build url config
        let url = match network {
            MAINNET => MAINNET_URL,
            SEPOLIA => SEPOLIA_URL,
            _ => {
                return Err(eyre!(
                    "Unknown network: {}. Only mainnet and sepolia are supported.",
                    network
                ));
            }
        };

        let final_url = Url::from_str(url).map_err(|e| eyre!("Failed to parse URL: {}", e))?;

        let folder = self.temp_dir.path().to_owned().into_boxed_path();

        // set up the client
        let client = EraClient::new(Client::new(), final_url, folder);

        // set up the file list, required before we can download files
        client.fetch_file_list().await.map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to fetch file list: {e}")))
        })?;

        // create an url for the file
        let file_url = Url::parse(&format!("{url}{filename}"))
            .map_err(|e| eyre!("Failed to parse file URL: {}", e))?;

        // download the file
        let mut client = client;
        let downloaded_path = client
            .download_to_file(file_url)
            .await
            .map_err(|e| eyre!("Failed to download file: {}", e))?;
        // update the cache
        {
            let mut cache = self.file_cache.lock().unwrap();
            cache.insert(filename.to_string(), downloaded_path.to_path_buf());
        }

        Ok(downloaded_path.to_path_buf())
    }

    /// open .era1 file, downloading it if necessary
    async fn open_era1_file(&self, filename: &str, network: &str) -> Result<Era1File> {
        let path = self.download_file(filename, network).await?;
        Era1Reader::open(&path, network).map_err(|e| eyre!("Failed to open Era1 file: {e}"))
    }
}

/// Open a test file by name,
/// downloading only if it is necessary
async fn open_test_file(
    file_path: &str,
    downloader: &Era1TestDownloader,
    network: &str,
) -> Result<Era1File> {
    let filename = Path::new(file_path)
        .file_name()
        .and_then(|os_str| os_str.to_str())
        .ok_or_else(|| eyre!("Invalid file path: {}", file_path))?;

    downloader.open_era1_file(filename, network).await
}
