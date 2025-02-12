//! Directory Manager downloads and manages files from the op-superchain-registry

use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio::fs as tokio_fs;
use zstd::{dict::DecoderDictionary, stream::read::Decoder};

/// Directory manager that handles caching and downloading of genesis files
#[derive(Debug)]
#[allow(dead_code)]
pub struct DirectoryManager {
    base_path: PathBuf,
}

#[allow(dead_code)]
impl DirectoryManager {
    const DICT_URL: &'static str = "https://raw.githubusercontent.com/ethereum-optimism/superchain-registry/main/superchain/extra/dictionary";
    const GENESIS_BASE_URL: &'static str = "https://raw.githubusercontent.com/ethereum-optimism/superchain-registry/main/superchain/extra/genesis";

    /// Create a new registry manager with the given base path
    pub(crate) fn new(base_path: PathBuf) -> Result<Self> {
        fs::create_dir_all(&base_path)?;
        Ok(Self { base_path })
    }

    /// Get the path to the dictionary file
    fn dictionary_path(&self) -> PathBuf {
        self.base_path.join("dictionary")
    }

    /// Get the path to a genesis file
    fn genesis_path(&self, network_type: &str, network: &str) -> PathBuf {
        self.base_path.join(network_type).join(format!("{}.json.zst", network))
    }

    /// Check if a file exists at the given path
    async fn file_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    /// Read file from the given path
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        tokio_fs::read(path).await.context("Failed to read file")
    }

    /// Save data to the given path
    async fn save_file(&self, path: &Path, data: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        tokio_fs::write(path, data).await.context("Failed to write file")
    }

    /// Download a file from the given URL
    async fn download_file(&self, url: &str, path: &Path) -> Result<Vec<u8>> {
        if self.file_exists(path).await {
            println!("Reading from cache: {}", path.display());
            return self.read_file(path).await;
        }

        println!("Downloading from URL: {}", url);
        let response = reqwest::get(url).await.context("Failed to download file")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to download: Status {}", response.status());
        }

        let bytes = response.bytes().await?.to_vec();
        self.save_file(path, &bytes).await?;

        Ok(bytes)
    }

    /// Download and update the dictionary
    async fn update_dictionary(&self) -> Result<Vec<u8>> {
        let path = self.dictionary_path();
        self.download_file(Self::DICT_URL, &path).await
    }

    /// Get genesis data for a network, downloading it if necessary
    pub(crate) async fn get_genesis(&self, network_type: &str, network: &str) -> Result<Value> {
        let dict_bytes = self.update_dictionary().await?;
        println!("Got dictionary: {} bytes", dict_bytes.len());

        let dictionary = DecoderDictionary::copy(&dict_bytes);

        let url = format!("{}/{}/{}.json.zst", Self::GENESIS_BASE_URL, network_type, network);
        let path = self.genesis_path(network_type, network);

        let compressed_bytes = self.download_file(&url, &path).await?;
        println!("Got genesis file: {} bytes", compressed_bytes.len());

        let decoder = Decoder::with_prepared_dictionary(&compressed_bytes[..], &dictionary)
            .context("Failed to create decoder with dictionary")?;

        println!("Parsing JSON...");
        let json: Value = serde_json::from_reader(decoder).context("Failed to parse JSON")?;

        Ok(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[tokio::test]
    async fn test_directory_manager() -> Result<()> {
        // Create a temporary directory for testing
        let test_dir = PathBuf::from("test-registry");
        let manager = DirectoryManager::new(test_dir)?;

        // Test downloading genesis data
        let json_data = manager.get_genesis("mainnet", "base").await?;
        assert!(json_data.is_object(), "Parsed JSON should be an object");

        // Test using cached data
        let cached_json_data = manager.get_genesis("mainnet", "base").await?;
        assert!(cached_json_data.is_object(), "Cached JSON should be an object");

        Ok(())
    }
}
