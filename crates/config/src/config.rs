//! Configuration files.
use reth_network_types::{PeersConfig, SessionsConfig};
use reth_prune_types::PruneModes;
use reth_stages_types::ExecutionStageThresholds;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

#[cfg(feature = "serde")]
const EXTENSION: &str = "toml";

/// The default prune block interval
pub const DEFAULT_BLOCK_INTERVAL: usize = 5;


/// Configuration for the reth node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Config {
    /// Configuration for each stage in the pipeline.
    // TODO(onbjerg): Can we make this easier to maintain when we add/remove stages?
    pub stages: StageConfig,
    /// Configuration for pruning.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub prune: Option<PruneConfig>,
    /// Configuration for the discovery service.
    pub peers: PeersConfig,
    /// Configuration for peer sessions.
    pub sessions: SessionsConfig,
}

impl Config {
    /// Sets the pruning configuration.
    pub fn update_prune_config(&mut self, prune_config: PruneConfig) {
        self.prune = Some(prune_config);
    }
}

#[cfg(feature = "serde")]
impl Config {
    /// Load a [`Config`] from a specified path.
    ///
    /// A new configuration file is created with default values if none
    /// exists.
    pub fn from_path(path: impl AsRef<Path>) -> eyre::Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(cfg_string) => {
                toml::from_str(&cfg_string).map_err(|e| eyre::eyre!("Failed to parse TOML: {e}"))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| eyre::eyre!("Failed to create directory: {e}"))?;
                }
                let cfg = Self::default();
                let s = toml::to_string_pretty(&cfg)
                    .map_err(|e| eyre::eyre!("Failed to serialize to TOML: {e}"))?;
                std::fs::write(path, s)
                    .map_err(|e| eyre::eyre!("Failed to write configuration file: {e}"))?;
                Ok(cfg)
            }
            Err(e) => Err(eyre::eyre!("Failed to load configuration: {e}")),
        }
    }

    /// Returns the [`PeersConfig`] for the node.
    ///
    /// If a peers file is provided, the basic nodes from the file are added to the configuration.
    pub fn peers_config_with_basic_nodes_from_file(
        &self,
        peers_file: Option<&Path>,
    ) -> PeersConfig {
        self.peers
            .clone()
            .with_basic_nodes_from_file(peers_file)
            .unwrap_or_else(|_| self.peers.clone())
    }

    /// Save the configuration to toml file.
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if path.extension() != Some(std::ffi::OsStr::new(EXTENSION)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("reth config file extension must be '{EXTENSION}'"),
            ));
        }

        std::fs::write(
            path,
            toml::to_string(self)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?,
        )
    }
}

/// Configuration for each stage in the pipeline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct StageConfig {
    /// ERA stage configuration.
    pub era: EraConfig,
    /// Header stage configuration.
    pub headers: HeadersConfig,
    /// Body stage configuration.
    pub bodies: BodiesConfig,
    /// Sender Recovery stage configuration.
    pub sender_recovery: SenderRecoveryConfig,
    /// Execution stage configuration.
    pub execution: ExecutionConfig,
    /// Prune stage configuration.
    pub prune: PruneStageConfig,
    /// Account Hashing stage configuration.
    pub account_hashing: HashingConfig,
    /// Storage Hashing stage configuration.
    pub storage_hashing: HashingConfig,
    /// Merkle stage configuration.
    pub merkle: MerkleConfig,
    /// Transaction Lookup stage configuration.
    pub transaction_lookup: TransactionLookupConfig,
    /// Index Account History stage configuration.
    pub index_account_history: IndexHistoryConfig,
    /// Index Storage History stage configuration.
    pub index_storage_history: IndexHistoryConfig,
    /// Snap Sync stage configuration.
    pub snap_sync: SnapSyncConfig,
    /// Common ETL related configuration.
    pub etl: EtlConfig,
}

impl StageConfig {
    /// The highest threshold (in number of blocks) for switching between incremental and full
    /// calculations across `MerkleStage`, `AccountHashingStage` and `StorageHashingStage`. This is
    /// required to figure out if can prune or not changesets on subsequent pipeline runs during
    /// `ExecutionStage`
    pub fn execution_external_clean_threshold(&self) -> u64 {
        self.merkle
            .incremental_threshold
            .max(self.account_hashing.clean_threshold)
            .max(self.storage_hashing.clean_threshold)
    }
}

/// ERA stage configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct EraConfig {
    /// Path to a local directory where ERA1 files are located.
    ///
    /// Conflicts with `url`.
    pub path: Option<PathBuf>,
    /// The base URL of an ERA1 file host to download from.
    ///
    /// Conflicts with `path`.
    pub url: Option<Url>,
    /// Path to a directory where files downloaded from `url` will be stored until processed.
    ///
    /// Required for `url`.
    pub folder: Option<PathBuf>,
}

impl EraConfig {
    /// Sets `folder` for temporary downloads as a directory called "era" inside `dir`.
    pub fn with_datadir(mut self, dir: impl AsRef<Path>) -> Self {
        self.folder = Some(dir.as_ref().join("era"));
        self
    }
}

/// Header stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct HeadersConfig {
    /// The maximum number of requests to send concurrently.
    ///
    /// Default: 100
    pub downloader_max_concurrent_requests: usize,
    /// The minimum number of requests to send concurrently.
    ///
    /// Default: 5
    pub downloader_min_concurrent_requests: usize,
    /// Maximum amount of responses to buffer internally.
    /// The response contains multiple headers.
    pub downloader_max_buffered_responses: usize,
    /// The maximum number of headers to request from a peer at a time.
    pub downloader_request_limit: u64,
    /// The maximum number of headers to download before committing progress to the database.
    pub commit_threshold: u64,
}

impl Default for HeadersConfig {
    fn default() -> Self {
        Self {
            commit_threshold: 10_000,
            downloader_request_limit: 1_000,
            downloader_max_concurrent_requests: 100,
            downloader_min_concurrent_requests: 5,
            downloader_max_buffered_responses: 100,
        }
    }
}

/// Body stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct BodiesConfig {
    /// The batch size of non-empty blocks per one request
    ///
    /// Default: 200
    pub downloader_request_limit: u64,
    /// The maximum number of block bodies returned at once from the stream
    ///
    /// Default: `1_000`
    pub downloader_stream_batch_size: usize,
    /// The size of the internal block buffer in bytes.
    ///
    /// Default: 2GB
    pub downloader_max_buffered_blocks_size_bytes: usize,
    /// The minimum number of requests to send concurrently.
    ///
    /// Default: 5
    pub downloader_min_concurrent_requests: usize,
    /// The maximum number of requests to send concurrently.
    /// This is equal to the max number of peers.
    ///
    /// Default: 100
    pub downloader_max_concurrent_requests: usize,
}

impl Default for BodiesConfig {
    fn default() -> Self {
        Self {
            downloader_request_limit: 200,
            downloader_stream_batch_size: 1_000,
            downloader_max_buffered_blocks_size_bytes: 2 * 1024 * 1024 * 1024, // ~2GB
            downloader_min_concurrent_requests: 5,
            downloader_max_concurrent_requests: 100,
        }
    }
}

/// Sender recovery stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct SenderRecoveryConfig {
    /// The maximum number of transactions to process before committing progress to the database.
    pub commit_threshold: u64,
}

impl Default for SenderRecoveryConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000_000 }
    }
}

/// Snap Sync stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct SnapSyncConfig {
    /// Enable snap sync stage.
    pub enabled: bool,
    /// Max account ranges per execution.
    pub max_ranges_per_execution: usize,
    /// Max response bytes per request.
    pub max_response_bytes: u64,
    /// Timeout for peer requests (seconds).
    pub request_timeout_seconds: u64,
    /// Range size for account hash ranges (in hash space units).
    /// Larger values = fewer requests but more data per request.
    pub range_size: u64,
    /// Maximum number of retries for failed requests.
    pub max_retries: u32,
}

impl Default for SnapSyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_ranges_per_execution: 100,
            max_response_bytes: 2 * 1024 * 1024, // 2MB
            request_timeout_seconds: 30,
            range_size: 0x10, // 16 hash values (very small default for testing)
            max_retries: 3, // 3 retries by default
        }
    }
}

/// Execution stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ExecutionConfig {
    /// The maximum number of blocks to process before the execution stage commits.
    pub max_blocks: Option<u64>,
    /// The maximum number of state changes to keep in memory before the execution stage commits.
    pub max_changes: Option<u64>,
    /// The maximum cumulative amount of gas to process before the execution stage commits.
    pub max_cumulative_gas: Option<u64>,
    /// The maximum time spent on blocks processing before the execution stage commits.
    #[cfg_attr(
        feature = "serde",
        serde(
            serialize_with = "humantime_serde::serialize",
            deserialize_with = "deserialize_duration"
        )
    )]
    pub max_duration: Option<Duration>,
    /// External clean threshold for execution stage.
    pub external_clean_threshold: u64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_blocks: Some(500_000),
            max_changes: Some(5_000_000),
            // 50k full blocks of 30M gas
            max_cumulative_gas: Some(30_000_000 * 50_000),
            // 10 minutes
            max_duration: Some(Duration::from_secs(10 * 60)),
            external_clean_threshold: 100_000,
        }
    }
}

impl From<ExecutionConfig> for ExecutionStageThresholds {
    fn from(config: ExecutionConfig) -> Self {
        Self {
            max_blocks: config.max_blocks,
            max_changes: config.max_changes,
            max_cumulative_gas: config.max_cumulative_gas,
            max_duration: config.max_duration,
        }
    }
}

/// Prune stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PruneStageConfig {
    /// The maximum number of entries to prune before committing progress to the database.
    pub commit_threshold: usize,
}

impl Default for PruneStageConfig {
    fn default() -> Self {
        Self { commit_threshold: 1_000_000 }
    }
}

/// Hashing stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct HashingConfig {
    /// The threshold (in number of blocks) for switching between
    /// incremental hashing and full hashing.
    pub clean_threshold: u64,
    /// The maximum number of entities to process before committing progress to the database.
    pub commit_threshold: u64,
}

impl Default for HashingConfig {
    fn default() -> Self {
        Self { clean_threshold: 500_000, commit_threshold: 100_000 }
    }
}

/// Merkle stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct MerkleConfig {
    /// The number of blocks we will run the incremental root method for when we are catching up on
    /// the merkle stage for a large number of blocks.
    ///
    /// When we are catching up for a large number of blocks, we can only run the incremental root
    /// for a limited number of blocks, otherwise the incremental root method may cause the node to
    /// OOM. This number determines how many blocks in a row we will run the incremental root
    /// method for.
    pub incremental_threshold: u64,
    /// The threshold (in number of blocks) for switching from incremental trie building of changes
    /// to whole rebuild.
    pub rebuild_threshold: u64,
}

impl Default for MerkleConfig {
    fn default() -> Self {
        Self { incremental_threshold: 7_000, rebuild_threshold: 100_000 }
    }
}

/// Transaction Lookup stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TransactionLookupConfig {
    /// The maximum number of transactions to process before writing to disk.
    pub chunk_size: u64,
}

impl Default for TransactionLookupConfig {
    fn default() -> Self {
        Self { chunk_size: 5_000_000 }
    }
}

/// Common ETL related configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct EtlConfig {
    /// Data directory where temporary files are created.
    pub dir: Option<PathBuf>,
    /// The maximum size in bytes of data held in memory before being flushed to disk as a file.
    pub file_size: usize,
}

impl Default for EtlConfig {
    fn default() -> Self {
        Self { dir: None, file_size: Self::default_file_size() }
    }
}

impl EtlConfig {
    /// Creates an ETL configuration
    pub const fn new(dir: Option<PathBuf>, file_size: usize) -> Self {
        Self { dir, file_size }
    }

    /// Return default ETL directory from datadir path.
    pub fn from_datadir(path: &Path) -> PathBuf {
        path.join("etl-tmp")
    }

    /// Default size in bytes of data held in memory before being flushed to disk as a file.
    pub const fn default_file_size() -> usize {
        // 500 MB
        500 * (1024 * 1024)
    }
}

/// History stage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct IndexHistoryConfig {
    /// The maximum number of blocks to process before committing progress to the database.
    pub commit_threshold: u64,
}

impl Default for IndexHistoryConfig {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
    }
}

/// Pruning configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PruneConfig {
    /// Minimum pruning interval measured in blocks.
    pub block_interval: usize,
    /// Pruning configuration for every part of the data that can be pruned.
    #[cfg_attr(feature = "serde", serde(alias = "parts"))]
    pub segments: PruneModes,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self { block_interval: DEFAULT_BLOCK_INTERVAL, segments: PruneModes::none() }
    }
}

impl PruneConfig {
    /// Returns whether there is any kind of receipt pruning configuration.
    pub fn has_receipts_pruning(&self) -> bool {
        self.segments.receipts.is_some() || !self.segments.receipts_log_filter.is_empty()
    }

    /// Merges another `PruneConfig` into this one, taking values from the other config if and only
    /// if the corresponding value in this config is not set.
    pub fn merge(&mut self, other: Option<Self>) {
        let Some(other) = other else { return };
        let Self {
            block_interval,
            segments:
                PruneModes {
                    sender_recovery,
                    transaction_lookup,
                    receipts,
                    account_history,
                    storage_history,
                    bodies_history,
                    receipts_log_filter,
                },
        } = other;

        // Merge block_interval, only update if it's the default interval
        if self.block_interval == DEFAULT_BLOCK_INTERVAL {
            self.block_interval = block_interval;
        }

        // Merge the various segment prune modes
        self.segments.sender_recovery = self.segments.sender_recovery.or(sender_recovery);
        self.segments.transaction_lookup = self.segments.transaction_lookup.or(transaction_lookup);
        self.segments.receipts = self.segments.receipts.or(receipts);
        self.segments.account_history = self.segments.account_history.or(account_history);
        self.segments.storage_history = self.segments.storage_history.or(storage_history);
        self.segments.bodies_history = self.segments.bodies_history.or(bodies_history);

        if self.segments.receipts_log_filter.0.is_empty() && !receipts_log_filter.0.is_empty() {
            self.segments.receipts_log_filter = receipts_log_filter;
        }
    }
}

/// Helper type to support older versions of Duration deserialization.
#[cfg(feature = "serde")]
fn deserialize_duration<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum AnyDuration {
        #[serde(deserialize_with = "humantime_serde::deserialize")]
        Human(Option<Duration>),
        Duration(Option<Duration>),
    }

    <AnyDuration as serde::Deserialize>::deserialize(deserializer).map(|d| match d {
        AnyDuration::Human(duration) | AnyDuration::Duration(duration) => duration,
    })
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::{Config, EXTENSION};
    use crate::PruneConfig;
    use alloy_primitives::Address;
    use reth_network_peers::TrustedPeer;
    use reth_prune_types::{PruneMode, PruneModes, ReceiptsLogPruneConfig};
    use std::{collections::BTreeMap, path::Path, str::FromStr, time::Duration};

    fn with_tempdir(filename: &str, proc: fn(&std::path::Path)) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join(filename).with_extension(EXTENSION);

        proc(&config_path);

        temp_dir.close().unwrap()
    }

    /// Run a test function with a temporary config path as fixture.
    fn with_config_path(test_fn: fn(&Path)) {
        // Create a temporary directory for the config file
        let config_dir = tempfile::tempdir().expect("creating test fixture failed");
        // Create the config file path
        let config_path =
            config_dir.path().join("example-app").join("example-config").with_extension("toml");
        // Run the test function with the config path
        test_fn(&config_path);
        config_dir.close().expect("removing test fixture failed");
    }

    #[test]
    fn test_load_path_works() {
        with_config_path(|path| {
            let config = Config::from_path(path).expect("load_path failed");
            assert_eq!(config, Config::default());
        })
    }

    #[test]
    fn test_load_path_reads_existing_config() {
        with_config_path(|path| {
            let config = Config::default();

            // Create the parent directory if it doesn't exist
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("Failed to create directories");
            }

            // Write the config to the file
            std::fs::write(path, toml::to_string(&config).unwrap())
                .expect("Failed to write config");

            // Load the config from the file and compare it
            let loaded = Config::from_path(path).expect("load_path failed");
            assert_eq!(config, loaded);
        })
    }

    #[test]
    fn test_load_path_fails_on_invalid_toml() {
        with_config_path(|path| {
            let invalid_toml = "invalid toml data";

            // Create the parent directory if it doesn't exist
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("Failed to create directories");
            }

            // Write invalid TOML data to the file
            std::fs::write(path, invalid_toml).expect("Failed to write invalid TOML");

            // Attempt to load the config should fail
            let result = Config::from_path(path);
            assert!(result.is_err());
        })
    }

    #[test]
    fn test_load_path_creates_directory_if_not_exists() {
        with_config_path(|path| {
            // Ensure the directory does not exist
            let parent = path.parent().unwrap();
            assert!(!parent.exists());

            // Load the configuration, which should create the directory and a default config file
            let config = Config::from_path(path).expect("load_path failed");
            assert_eq!(config, Config::default());

            // The directory and file should now exist
            assert!(parent.exists());
            assert!(path.exists());
        });
    }

    #[test]
    fn test_store_config() {
        with_tempdir("config-store-test", |config_path| {
            let config = Config::default();
            std::fs::write(
                config_path,
                toml::to_string(&config).expect("Failed to serialize config"),
            )
            .expect("Failed to write config file");
        })
    }

    #[test]
    fn test_store_config_method() {
        with_tempdir("config-store-test-method", |config_path| {
            let config = Config::default();
            config.save(config_path).expect("Failed to store config");
        })
    }

    #[test]
    fn test_load_config() {
        with_tempdir("config-load-test", |config_path| {
            let config = Config::default();

            // Write the config to a file
            std::fs::write(
                config_path,
                toml::to_string(&config).expect("Failed to serialize config"),
            )
            .expect("Failed to write config file");

            // Load the config from the file
            let loaded_config = Config::from_path(config_path).unwrap();

            // Compare the loaded config with the original config
            assert_eq!(config, loaded_config);
        })
    }

    #[test]
    fn test_load_execution_stage() {
        with_tempdir("config-load-test", |config_path| {
            let mut config = Config::default();
            config.stages.execution.max_duration = Some(Duration::from_secs(10 * 60));

            // Write the config to a file
            std::fs::write(
                config_path,
                toml::to_string(&config).expect("Failed to serialize config"),
            )
            .expect("Failed to write config file");

            // Load the config from the file
            let loaded_config = Config::from_path(config_path).unwrap();

            // Compare the loaded config with the original config
            assert_eq!(config, loaded_config);
        })
    }

    // ensures config deserialization is backwards compatible
    #[test]
    fn test_backwards_compatibility() {
        let alpha_0_0_8 = r"#
[stages.headers]
downloader_max_concurrent_requests = 100
downloader_min_concurrent_requests = 5
downloader_max_buffered_responses = 100
downloader_request_limit = 1000
commit_threshold = 10000

[stages.bodies]
downloader_request_limit = 200
downloader_stream_batch_size = 1000
downloader_max_buffered_blocks_size_bytes = 2147483648
downloader_min_concurrent_requests = 5
downloader_max_concurrent_requests = 100

[stages.sender_recovery]
commit_threshold = 5000000

[stages.execution]
max_blocks = 500000
max_changes = 5000000

[stages.account_hashing]
clean_threshold = 500000
commit_threshold = 100000

[stages.storage_hashing]
clean_threshold = 500000
commit_threshold = 100000

[stages.merkle]
clean_threshold = 50000

[stages.transaction_lookup]
chunk_size = 5000000

[stages.index_account_history]
commit_threshold = 100000

[stages.index_storage_history]
commit_threshold = 100000

[peers]
refill_slots_interval = '1s'
trusted_nodes = []
connect_trusted_nodes_only = false
max_backoff_count = 5
ban_duration = '12h'

[peers.connection_info]
max_outbound = 100
max_inbound = 30

[peers.reputation_weights]
bad_message = -16384
bad_block = -16384
bad_transactions = -16384
already_seen_transactions = 0
timeout = -4096
bad_protocol = -2147483648
failed_to_connect = -25600
dropped = -4096

[peers.backoff_durations]
low = '30s'
medium = '3m'
high = '15m'
max = '1h'

[sessions]
session_command_buffer = 32
session_event_buffer = 260

[sessions.limits]

[sessions.initial_internal_request_timeout]
secs = 20
nanos = 0

[sessions.protocol_breach_request_timeout]
secs = 120
nanos = 0

[prune]
block_interval = 5

[prune.parts]
sender_recovery = { distance = 16384 }
transaction_lookup = 'full'
receipts = { before = 1920000 }
account_history = { distance = 16384 }
storage_history = { distance = 16384 }
[prune.parts.receipts_log_filter]
'0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48' = { before = 17000000 }
'0xdac17f958d2ee523a2206206994597c13d831ec7' = { distance = 1000 }
#";
        let _conf: Config = toml::from_str(alpha_0_0_8).unwrap();

        let alpha_0_0_11 = r"#
[prune.segments]
sender_recovery = { distance = 16384 }
transaction_lookup = 'full'
receipts = { before = 1920000 }
account_history = { distance = 16384 }
storage_history = { distance = 16384 }
[prune.segments.receipts_log_filter]
'0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48' = { before = 17000000 }
'0xdac17f958d2ee523a2206206994597c13d831ec7' = { distance = 1000 }
#";
        let _conf: Config = toml::from_str(alpha_0_0_11).unwrap();

        let alpha_0_0_18 = r"#
[stages.headers]
downloader_max_concurrent_requests = 100
downloader_min_concurrent_requests = 5
downloader_max_buffered_responses = 100
downloader_request_limit = 1000
commit_threshold = 10000

[stages.total_difficulty]
commit_threshold = 100000

[stages.bodies]
downloader_request_limit = 200
downloader_stream_batch_size = 1000
downloader_max_buffered_blocks_size_bytes = 2147483648
downloader_min_concurrent_requests = 5
downloader_max_concurrent_requests = 100

[stages.sender_recovery]
commit_threshold = 5000000

[stages.execution]
max_blocks = 500000
max_changes = 5000000
max_cumulative_gas = 1500000000000
[stages.execution.max_duration]
secs = 600
nanos = 0

[stages.account_hashing]
clean_threshold = 500000
commit_threshold = 100000

[stages.storage_hashing]
clean_threshold = 500000
commit_threshold = 100000

[stages.merkle]
clean_threshold = 50000

[stages.transaction_lookup]
commit_threshold = 5000000

[stages.index_account_history]
commit_threshold = 100000

[stages.index_storage_history]
commit_threshold = 100000

[peers]
refill_slots_interval = '5s'
trusted_nodes = []
connect_trusted_nodes_only = false
max_backoff_count = 5
ban_duration = '12h'

[peers.connection_info]
max_outbound = 100
max_inbound = 30
max_concurrent_outbound_dials = 10

[peers.reputation_weights]
bad_message = -16384
bad_block = -16384
bad_transactions = -16384
already_seen_transactions = 0
timeout = -4096
bad_protocol = -2147483648
failed_to_connect = -25600
dropped = -4096
bad_announcement = -1024

[peers.backoff_durations]
low = '30s'
medium = '3m'
high = '15m'
max = '1h'

[sessions]
session_command_buffer = 32
session_event_buffer = 260

[sessions.limits]

[sessions.initial_internal_request_timeout]
secs = 20
nanos = 0

[sessions.protocol_breach_request_timeout]
secs = 120
nanos = 0
#";
        let conf: Config = toml::from_str(alpha_0_0_18).unwrap();
        assert_eq!(conf.stages.execution.max_duration, Some(Duration::from_secs(10 * 60)));

        let alpha_0_0_19 = r"#
[stages.headers]
downloader_max_concurrent_requests = 100
downloader_min_concurrent_requests = 5
downloader_max_buffered_responses = 100
downloader_request_limit = 1000
commit_threshold = 10000

[stages.total_difficulty]
commit_threshold = 100000

[stages.bodies]
downloader_request_limit = 200
downloader_stream_batch_size = 1000
downloader_max_buffered_blocks_size_bytes = 2147483648
downloader_min_concurrent_requests = 5
downloader_max_concurrent_requests = 100

[stages.sender_recovery]
commit_threshold = 5000000

[stages.execution]
max_blocks = 500000
max_changes = 5000000
max_cumulative_gas = 1500000000000
max_duration = '10m'

[stages.account_hashing]
clean_threshold = 500000
commit_threshold = 100000

[stages.storage_hashing]
clean_threshold = 500000
commit_threshold = 100000

[stages.merkle]
clean_threshold = 50000

[stages.transaction_lookup]
commit_threshold = 5000000

[stages.index_account_history]
commit_threshold = 100000

[stages.index_storage_history]
commit_threshold = 100000

[peers]
refill_slots_interval = '5s'
trusted_nodes = []
connect_trusted_nodes_only = false
max_backoff_count = 5
ban_duration = '12h'

[peers.connection_info]
max_outbound = 100
max_inbound = 30
max_concurrent_outbound_dials = 10

[peers.reputation_weights]
bad_message = -16384
bad_block = -16384
bad_transactions = -16384
already_seen_transactions = 0
timeout = -4096
bad_protocol = -2147483648
failed_to_connect = -25600
dropped = -4096
bad_announcement = -1024

[peers.backoff_durations]
low = '30s'
medium = '3m'
high = '15m'
max = '1h'

[sessions]
session_command_buffer = 32
session_event_buffer = 260

[sessions.limits]

[sessions.initial_internal_request_timeout]
secs = 20
nanos = 0

[sessions.protocol_breach_request_timeout]
secs = 120
nanos = 0
#";
        let _conf: Config = toml::from_str(alpha_0_0_19).unwrap();
    }

    // ensures prune config deserialization is backwards compatible
    #[test]
    fn test_backwards_compatibility_prune_full() {
        let s = r"#
[prune]
block_interval = 5

[prune.segments]
sender_recovery = { distance = 16384 }
transaction_lookup = 'full'
receipts = { distance = 16384 }
#";
        let _conf: Config = toml::from_str(s).unwrap();

        let s = r"#
[prune]
block_interval = 5

[prune.segments]
sender_recovery = { distance = 16384 }
transaction_lookup = 'full'
receipts = 'full'
#";
        let err = toml::from_str::<Config>(s).unwrap_err().to_string();
        assert!(err.contains("invalid value: string \"full\""), "{}", err);
    }

    #[test]
    fn test_prune_config_merge() {
        let mut config1 = PruneConfig {
            block_interval: 5,
            segments: PruneModes {
                sender_recovery: Some(PruneMode::Full),
                transaction_lookup: None,
                receipts: Some(PruneMode::Distance(1000)),
                account_history: None,
                storage_history: Some(PruneMode::Before(5000)),
                bodies_history: None,
                receipts_log_filter: ReceiptsLogPruneConfig(BTreeMap::from([(
                    Address::random(),
                    PruneMode::Full,
                )])),
            },
        };

        let config2 = PruneConfig {
            block_interval: 10,
            segments: PruneModes {
                sender_recovery: Some(PruneMode::Distance(500)),
                transaction_lookup: Some(PruneMode::Full),
                receipts: Some(PruneMode::Full),
                account_history: Some(PruneMode::Distance(2000)),
                storage_history: Some(PruneMode::Distance(3000)),
                bodies_history: None,
                receipts_log_filter: ReceiptsLogPruneConfig(BTreeMap::from([
                    (Address::random(), PruneMode::Distance(1000)),
                    (Address::random(), PruneMode::Before(2000)),
                ])),
            },
        };

        let original_filter = config1.segments.receipts_log_filter.clone();
        config1.merge(Some(config2));

        // Check that the configuration has been merged. Any configuration present in config1
        // should not be overwritten by config2
        assert_eq!(config1.block_interval, 10);
        assert_eq!(config1.segments.sender_recovery, Some(PruneMode::Full));
        assert_eq!(config1.segments.transaction_lookup, Some(PruneMode::Full));
        assert_eq!(config1.segments.receipts, Some(PruneMode::Distance(1000)));
        assert_eq!(config1.segments.account_history, Some(PruneMode::Distance(2000)));
        assert_eq!(config1.segments.storage_history, Some(PruneMode::Before(5000)));
        assert_eq!(config1.segments.receipts_log_filter, original_filter);
    }

    #[test]
    fn test_conf_trust_nodes_only() {
        let trusted_nodes_only = r"#
[peers]
trusted_nodes_only = true
#";
        let conf: Config = toml::from_str(trusted_nodes_only).unwrap();
        assert!(conf.peers.trusted_nodes_only);

        let trusted_nodes_only = r"#
[peers]
connect_trusted_nodes_only = true
#";
        let conf: Config = toml::from_str(trusted_nodes_only).unwrap();
        assert!(conf.peers.trusted_nodes_only);
    }

    #[test]
    fn test_can_support_dns_in_trusted_nodes() {
        let reth_toml = r#"
    [peers]
    trusted_nodes = [
        "enode://0401e494dbd0c84c5c0f72adac5985d2f2525e08b68d448958aae218f5ac8198a80d1498e0ebec2ce38b1b18d6750f6e61a56b4614c5a6c6cf0981c39aed47dc@34.159.32.127:30303",
        "enode://e9675164b5e17b9d9edf0cc2bd79e6b6f487200c74d1331c220abb5b8ee80c2eefbf18213989585e9d0960683e819542e11d4eefb5f2b4019e1e49f9fd8fff18@berav2-bootnode.staketab.org:30303"
    ]
    "#;

        let conf: Config = toml::from_str(reth_toml).unwrap();
        assert_eq!(conf.peers.trusted_nodes.len(), 2);

        let expected_enodes = vec![
            "enode://0401e494dbd0c84c5c0f72adac5985d2f2525e08b68d448958aae218f5ac8198a80d1498e0ebec2ce38b1b18d6750f6e61a56b4614c5a6c6cf0981c39aed47dc@34.159.32.127:30303",
            "enode://e9675164b5e17b9d9edf0cc2bd79e6b6f487200c74d1331c220abb5b8ee80c2eefbf18213989585e9d0960683e819542e11d4eefb5f2b4019e1e49f9fd8fff18@berav2-bootnode.staketab.org:30303",
        ];

        for enode in expected_enodes {
            let node = TrustedPeer::from_str(enode).unwrap();
            assert!(conf.peers.trusted_nodes.contains(&node));
        }
    }
}
