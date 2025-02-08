use reqwest;
use serde_json::Value;
use zstd::stream::read::Decoder;
use zstd::dict::DecoderDictionary;
use anyhow::{Result, Context};

async fn download_file(url: &str) -> Result<Vec<u8>> {
    println!("Downloading from URL: {}", url);
    let response = reqwest::get(url)
        .await
        .context("Failed to download file")?;
    
    if !response.status().is_success() {
        anyhow::bail!("Failed to download: Status {}", response.status());
    }
    
    Ok(response.bytes().await?.to_vec())
}

async fn download_and_parse_zst(url: &str, dict_url: &str) -> Result<Value> {
    // Download the dictionary
    let dict_bytes = download_file(dict_url).await?;
    println!("Downloaded dictionary: {} bytes", dict_bytes.len());
    
    let dictionary = DecoderDictionary::copy(&dict_bytes);
    
    // Download the main file
    let compressed_bytes = download_file(url).await?;
    println!("Downloaded compressed file: {} bytes", compressed_bytes.len());
    
    // Create decoder with dictionary
    let decoder = Decoder::with_prepared_dictionary(&compressed_bytes[..], &dictionary)
        .context("Failed to create decoder with dictionary")?;
    
    // Parse JSON
    println!("Parsing JSON...");
    let json: Value = serde_json::from_reader(decoder)
        .context("Failed to parse JSON")?;
    
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[tokio::test]
    async fn test_download_and_parse_genesis() -> Result<()> {
        let file_url = "https://raw.githubusercontent.com/ethereum-optimism/superchain-registry/main/superchain/extra/genesis/mainnet/arena-z.json.zst";
        let dict_url = "https://raw.githubusercontent.com/ethereum-optimism/superchain-registry/main/superchain/extra/dictionary";
        
        let json_data = download_and_parse_zst(file_url, dict_url).await?;
        
        assert!(json_data.is_object(), "Parsed JSON should be an object");
        
        Ok(())
    }
}