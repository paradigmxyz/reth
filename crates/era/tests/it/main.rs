//! Root
//! Includes common helpers for integration tests
//!
//! These tests use the `reth-era-downloader` client to download `.era1` files temporarily
//! and verify that we can correctly read and decompress their data.
//!
//! Files are downloaded from [`MAINNET_URL`] and [`SEPOLIA_URL`].

use reqwest::{Client, Url};
use reth_era::{
    common::file_ops::FileReader,
    e2s::error::E2sError,
    era::file::{EraFile, EraReader},
    era1::file::{Era1File, Era1Reader},
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

mod era;
mod era1;

const fn main() {}

/// Mainnet network name
const MAINNET: &str = "mainnet";
/// Default mainnet url
/// for downloading mainnet `.era1` files
const ERA1_MAINNET_URL: &str = "https://era.ithaca.xyz/era1/";

/// Succinct list of mainnet files we want to download
/// from <https://era.ithaca.xyz/era1/>
/// for testing purposes
const ERA1_MAINNET_FILES_NAMES: [&str; 8] = [
    "mainnet-00000-5ec1ffb8.era1",
    "mainnet-00003-d8b8a40b.era1",
    "mainnet-00151-e322efe1.era1",
    "mainnet-00293-0d6c5812.era1",
    "mainnet-00443-ea71b6f9.era1",
    "mainnet-01367-d7efc68f.era1",
    "mainnet-01610-99fdde4b.era1",
    "mainnet-01895-3f81607c.era1",
];

/// Sepolia network name
const SEPOLIA: &str = "sepolia";

/// Default sepolia url
/// for downloading sepolia `.era1` files
const ERA1_SEPOLIA_URL: &str = "https://era.ithaca.xyz/sepolia-era1/";

/// Succinct list of sepolia files we want to download
/// from <https://era.ithaca.xyz/sepolia-era1/>
/// for testing purposes
const ERA1_SEPOLIA_FILES_NAMES: [&str; 4] = [
    "sepolia-00000-643a00f7.era1",
    "sepolia-00074-0e81003c.era1",
    "sepolia-00173-b6924da5.era1",
    "sepolia-00182-a4f0a8a1.era1 ",
];

const HOODI: &str = "hoodi";

/// Default sepolia url
/// for downloading sepolia `.era1` files
const ERA_HOODI_URL: &str = "https://hoodi.era.nimbus.team/";

/// Succinct list of sepolia files we want to download
/// from <https://hoodi.era.nimbus.team/>
/// for testing purposes
const ERA_HOODI_FILES_NAMES: [&str; 4] = [
    "hoodi-00000-212f13fc.era",
    "hoodi-00021-857e418b.era",
    "hoodi-00175-202aaa6d.era",
    "hoodi-00201-0d521fc8.era",
];

/// Utility for downloading `.era1` files for tests
/// in a temporary directory
/// and caching them in memory
#[derive(Debug)]
struct EraTestDownloader {
    /// Temporary directory for storing downloaded files
    temp_dir: TempDir,
    /// Cache mapping file names to their paths
    file_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl EraTestDownloader {
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
        self.validate_filename(filename, network)?;

        let url = self.get_network_url(network)?;
        let final_url = Url::from_str(url).map_err(|e| eyre!("Failed to parse URL: {}", e))?;

        let folder = self.temp_dir.path();

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

    /// Validate that filename is in the supported list for the network
    fn validate_filename(&self, filename: &str, network: &str) -> Result<()> {
        let is_valid = match network {
            MAINNET => ERA1_MAINNET_FILES_NAMES.contains(&filename),
            SEPOLIA => ERA1_SEPOLIA_FILES_NAMES.contains(&filename),
            HOODI => ERA_HOODI_FILES_NAMES.contains(&filename),
            _ => {
                return Err(eyre!(
                    "Unknown network: {}. Supported networks: {}, {}, {}",
                    network,
                    MAINNET,
                    SEPOLIA,
                    HOODI
                ));
            }
        };

        if !is_valid {
            let supported_files = match network {
                MAINNET => &ERA1_MAINNET_FILES_NAMES[..],
                SEPOLIA => &ERA1_SEPOLIA_FILES_NAMES[..],
                HOODI => &ERA_HOODI_FILES_NAMES[..],
                _ => &[],
            };

            return Err(eyre!(
                "Unknown file: '{}' for network '{}'. Supported files: {:?}",
                filename,
                network,
                supported_files
            ));
        }

        Ok(())
    }

    /// Get the base URL for a network
    fn get_network_url(&self, network: &str) -> Result<&'static str> {
        match network {
            MAINNET => Ok(ERA1_MAINNET_URL),
            SEPOLIA => Ok(ERA1_SEPOLIA_URL),
            HOODI => Ok(ERA_HOODI_URL),
            _ => Err(eyre!(
                "Unknown network: {}. Supported networks: {}, {}, {}",
                network,
                MAINNET,
                SEPOLIA,
                HOODI
            )),
        }
    }

    /// open .era1 file, downloading it if necessary
    async fn open_era1_file(&self, filename: &str, network: &str) -> Result<Era1File> {
        let path = self.download_file(filename, network).await?;
        Era1Reader::open(&path, network).map_err(|e| eyre!("Failed to open Era1 file: {e}"))
    }

    /// open .era file, downloading it if necessary
    #[allow(dead_code)]
    async fn open_era_file(&self, filename: &str, network: &str) -> Result<EraFile> {
        let path = self.download_file(filename, network).await?;
        EraReader::open(&path, network).map_err(|e| eyre!("Failed to open Era1 file: {e}"))
    }
}

/// Open a test era1 file by name, downloading only if it is necessary
async fn open_era1_test_file(
    file_path: &str,
    downloader: &EraTestDownloader,
    network: &str,
) -> Result<Era1File> {
    let filename = Path::new(file_path)
        .file_name()
        .and_then(|os_str| os_str.to_str())
        .ok_or_else(|| eyre!("Invalid file path: {}", file_path))?;

    downloader.open_era1_file(filename, network).await
}

/// Open a test era file by name, downloading only if it is necessary
#[allow(dead_code)]
async fn open_era_test_file(
    file_path: &str,
    downloader: &EraTestDownloader,
    network: &str,
) -> Result<EraFile> {
    let filename = Path::new(file_path)
        .file_name()
        .and_then(|os_str| os_str.to_str())
        .ok_or_else(|| eyre!("Invalid file path: {}", file_path))?;

    downloader.open_era_file(filename, network).await
}
