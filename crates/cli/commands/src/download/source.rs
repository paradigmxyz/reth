use super::{manifest::SnapshotManifest, progress::DownloadProgress, DownloadDefaults};
use eyre::{Result, WrapErr};
use reqwest::Client;
use reth_fs_util as fs;
use std::path::{Path, PathBuf};
use tracing::info;
use url::Url;

/// An entry from the snapshot discovery API listing.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotApiEntry {
    #[serde(deserialize_with = "deserialize_string_or_u64")]
    chain_id: u64,
    #[serde(deserialize_with = "deserialize_string_or_u64")]
    block: u64,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    metadata_url: String,
    #[serde(default)]
    size: u64,
}

impl SnapshotApiEntry {
    /// Returns whether this discovery entry points to a modular manifest.
    fn is_modular(&self) -> bool {
        self.metadata_url.ends_with("manifest.json")
    }
}

/// Discovers the latest snapshot manifest URL for the given chain from the snapshots API.
///
/// Queries the configured snapshot API and returns the manifest URL for the most
/// recent modular snapshot matching the requested chain.
pub(crate) async fn discover_manifest_url(chain_id: u64) -> Result<String> {
    let defaults = DownloadDefaults::get_global();
    let api_url = &*defaults.snapshot_api_url;

    info!(target: "reth::cli", %api_url, %chain_id, "Discovering latest snapshot manifest");

    let entries = fetch_snapshot_api_entries(chain_id).await?;
    let entry =
        entries.iter().filter(|s| s.is_modular()).max_by_key(|s| s.block).ok_or_else(|| {
            eyre::eyre!(
                "No modular snapshot manifest found for chain \
             {chain_id} at {api_url}\n\n\
             You can provide a manifest URL directly with --manifest-url, or\n\
             use a direct snapshot URL with -u from:\n\
             \t- {}\n\n\
             Use --list to see all available snapshots.",
                api_url.trim_end_matches("/api/snapshots"),
            )
        })?;

    info!(target: "reth::cli",
        block = entry.block,
        url = %entry.metadata_url,
        "Found latest snapshot manifest"
    );

    Ok(entry.metadata_url.clone())
}

/// Deserializes a JSON value that may be either a number or a string-encoded number.
fn deserialize_string_or_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = serde_json::Value::deserialize(deserializer)?;
    match &value {
        serde_json::Value::Number(n) => {
            n.as_u64().ok_or_else(|| serde::de::Error::custom("expected u64"))
        }
        serde_json::Value::String(s) => {
            s.parse::<u64>().map_err(|_| serde::de::Error::custom("expected numeric string"))
        }
        _ => Err(serde::de::Error::custom("expected number or string")),
    }
}

/// Fetches the full snapshot listing from the snapshots API, filtered by chain ID.
pub(crate) async fn fetch_snapshot_api_entries(chain_id: u64) -> Result<Vec<SnapshotApiEntry>> {
    let api_url = &*DownloadDefaults::get_global().snapshot_api_url;

    let entries: Vec<SnapshotApiEntry> = Client::new()
        .get(api_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .wrap_err_with(|| format!("Failed to fetch snapshot listing from {api_url}"))?
        .json()
        .await?;

    Ok(entries.into_iter().filter(|entry| entry.chain_id == chain_id).collect())
}

/// Prints a formatted table of available modular snapshots.
pub(crate) fn print_snapshot_listing(entries: &[SnapshotApiEntry], chain_id: u64) {
    let modular: Vec<_> = entries.iter().filter(|entry| entry.is_modular()).collect();

    let api_url = &*DownloadDefaults::get_global().snapshot_api_url;
    println!(
        "Available snapshots for chain {chain_id} ({}):\n",
        api_url.trim_end_matches("/api/snapshots"),
    );
    println!("{:<12}  {:>10}  {:<10}  {:>10}  MANIFEST URL", "DATE", "BLOCK", "PROFILE", "SIZE");
    println!("{}", "-".repeat(100));

    for entry in &modular {
        let date = entry.date.as_deref().unwrap_or("-");
        let profile = entry.profile.as_deref().unwrap_or("-");
        let size = if entry.size > 0 {
            DownloadProgress::format_size(entry.size)
        } else {
            "-".to_string()
        };

        println!(
            "{date:<12}  {:>10}  {profile:<10}  {size:>10}  {}",
            entry.block, entry.metadata_url
        );
    }

    if modular.is_empty() {
        println!("  (no modular snapshots found)");
    }

    println!(
        "\nTo download a specific snapshot, copy its manifest URL and run:\n  \
         reth download --manifest-url <URL>"
    );
}

/// Loads a manifest from an HTTP(S) URL, `file://` URL, or local path.
pub(crate) async fn fetch_manifest_from_source(source: &str) -> Result<SnapshotManifest> {
    if let Ok(parsed) = Url::parse(source) {
        return match parsed.scheme() {
            "http" | "https" => {
                let response = Client::new()
                    .get(source)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .wrap_err_with(|| {
                        let sources = DownloadDefaults::get_global()
                            .available_snapshots
                            .iter()
                            .map(|snapshot| format!("\t- {snapshot}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!(
                            "Failed to fetch snapshot manifest from {source}\n\n\
                             The manifest endpoint may not be available for this snapshot source.\n\
                             You can use a direct snapshot URL instead:\n\n\
                             \treth download -u <snapshot-url>\n\n\
                             Available snapshot sources:\n{sources}"
                        )
                    })?;
                Ok(response.json().await?)
            }
            "file" => {
                let path = parsed
                    .to_file_path()
                    .map_err(|_| eyre::eyre!("Invalid file:// manifest path: {source}"))?;
                let content = fs::read_to_string(path)?;
                Ok(serde_json::from_str(&content)?)
            }
            _ => Err(eyre::eyre!("Unsupported manifest URL scheme: {}", parsed.scheme())),
        };
    }

    let content = fs::read_to_string(source)?;
    Ok(serde_json::from_str(&content)?)
}

/// Resolves the base URL used to join relative archive paths in a manifest.
pub(crate) fn resolve_manifest_base_url(
    manifest: &SnapshotManifest,
    source: &str,
) -> Result<String> {
    if let Some(base_url) = manifest.base_url.as_deref() &&
        !base_url.is_empty()
    {
        return Ok(base_url.trim_end_matches('/').to_string());
    }

    if let Ok(mut url) = Url::parse(source) {
        if url.scheme() == "file" {
            let mut path = url
                .to_file_path()
                .map_err(|_| eyre::eyre!("Invalid file:// manifest path: {source}"))?;
            path.pop();
            let mut base = Url::from_directory_path(path)
                .map_err(|_| eyre::eyre!("Invalid manifest directory for source: {source}"))?
                .to_string();
            if base.ends_with('/') {
                base.pop();
            }
            return Ok(base);
        }

        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| eyre::eyre!("manifest_url must have a hierarchical path"))?;
            segments.pop_if_empty();
            segments.pop();
        }
        return Ok(url.as_str().trim_end_matches('/').to_string());
    }

    let path = Path::new(source);
    let manifest_dir = if path.is_absolute() {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    } else {
        let joined = std::env::current_dir()?.join(path);
        joined.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    };

    let mut base = Url::from_directory_path(&manifest_dir)
        .map_err(|_| eyre::eyre!("Invalid manifest directory: {}", manifest_dir.display()))?
        .to_string();
    if base.ends_with('/') {
        base.pop();
    }
    Ok(base)
}
