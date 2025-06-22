//! Cache related abstraction
use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, B256, U256};
use alloy_provider::network::TransactionResponse;
use parking_lot::RwLock;
use revm::{
    context::BlockEnv,
    context_interface::block::BlobExcessGasAndPrice,
    primitives::{
        map::{AddressHashMap, HashMap},
        KECCAK_EMPTY,
    },
    state::{Account, AccountInfo, AccountStatus},
    DatabaseCommit,
};
use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::BTreeSet,
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{instrument, trace, warn};
use url::Url;

pub type StorageInfo = HashMap<U256, U256>;

/// A shareable Block database
#[derive(Clone, Debug)]
pub struct BlockchainDb {
    /// Contains all the data
    db: Arc<MemDb>,
    /// metadata of the current config
    meta: Arc<RwLock<BlockchainDbMeta>>,
    /// the cache that can be flushed
    cache: Arc<JsonBlockCacheDB>,
}

impl BlockchainDb {
    /// Creates a new instance of the [BlockchainDb].
    ///
    /// If a `cache_path` is provided it attempts to load a previously stored [JsonBlockCacheData]
    /// and will try to use the cached entries it holds.
    ///
    /// This will return a new and empty [MemDb] if
    ///   - `cache_path` is `None`
    ///   - the file the `cache_path` points to, does not exist
    ///   - the file contains malformed data, or if it couldn't be read
    ///   - the provided `meta` differs from [BlockchainDbMeta] that's stored on disk
    pub fn new(meta: BlockchainDbMeta, cache_path: Option<PathBuf>) -> Self {
        Self::new_db(meta, cache_path, false)
    }

    /// Creates a new instance of the [BlockchainDb] and skips check when comparing meta
    /// This is useful for offline-start mode when we don't want to fetch metadata of `block`.
    ///
    /// if a `cache_path` is provided it attempts to load a previously stored [JsonBlockCacheData]
    /// and will try to use the cached entries it holds.
    ///
    /// This will return a new and empty [MemDb] if
    ///   - `cache_path` is `None`
    ///   - the file the `cache_path` points to, does not exist
    ///   - the file contains malformed data, or if it couldn't be read
    ///   - the provided `meta` differs from [BlockchainDbMeta] that's stored on disk
    pub fn new_skip_check(meta: BlockchainDbMeta, cache_path: Option<PathBuf>) -> Self {
        Self::new_db(meta, cache_path, true)
    }

    fn new_db(meta: BlockchainDbMeta, cache_path: Option<PathBuf>, skip_check: bool) -> Self {
        trace!(target: "forge::cache", cache=?cache_path, "initialising blockchain db");
        // read cache and check if metadata matches
        let cache = cache_path
            .as_ref()
            .and_then(|p| {
                JsonBlockCacheDB::load(p).ok().filter(|cache| {
                    if skip_check {
                        return true;
                    }
                    let mut existing = cache.meta().write();
                    existing.hosts.extend(meta.hosts.clone());
                    if meta != *existing {
                        warn!(target: "cache", "non-matching block metadata");
                        false
                    } else {
                        true
                    }
                })
            })
            .unwrap_or_else(|| JsonBlockCacheDB::new(Arc::new(RwLock::new(meta)), cache_path));

        Self { db: Arc::clone(cache.db()), meta: Arc::clone(cache.meta()), cache: Arc::new(cache) }
    }

    /// Returns the map that holds the account related info
    pub fn accounts(&self) -> &RwLock<AddressHashMap<AccountInfo>> {
        &self.db.accounts
    }

    /// Returns the map that holds the storage related info
    pub fn storage(&self) -> &RwLock<AddressHashMap<StorageInfo>> {
        &self.db.storage
    }

    /// Returns the map that holds all the block hashes
    pub fn block_hashes(&self) -> &RwLock<HashMap<U256, B256>> {
        &self.db.block_hashes
    }

    /// Returns the Env related metadata
    pub const fn meta(&self) -> &Arc<RwLock<BlockchainDbMeta>> {
        &self.meta
    }

    /// Returns the inner cache
    pub const fn cache(&self) -> &Arc<JsonBlockCacheDB> {
        &self.cache
    }

    /// Returns the underlying storage
    pub const fn db(&self) -> &Arc<MemDb> {
        &self.db
    }
}

/// relevant identifying markers in the context of [BlockchainDb]
#[derive(Clone, Debug, Eq, Serialize, Default)]
pub struct BlockchainDbMeta {
    /// The block environment
    pub block_env: BlockEnv,
    /// All the hosts used to connect to
    pub hosts: BTreeSet<String>,
}

impl BlockchainDbMeta {
    /// Creates a new instance
    pub fn new(block_env: BlockEnv, url: String) -> Self {
        let host = Url::parse(&url)
            .ok()
            .and_then(|url| url.host().map(|host| host.to_string()))
            .unwrap_or(url);

        Self { block_env, hosts: BTreeSet::from([host]) }
    }

    /// Sets the [BlockEnv] of this instance using the provided [alloy_rpc_types::Block]
    pub fn with_block<T: TransactionResponse, H: BlockHeader>(
        mut self,
        block: &alloy_rpc_types::Block<T, H>,
    ) -> Self {
        self.block_env = BlockEnv {
            number: block.header.number(),
            beneficiary: block.header.beneficiary(),
            timestamp: block.header.timestamp(),
            difficulty: U256::from(block.header.difficulty()),
            basefee: block.header.base_fee_per_gas().unwrap_or_default(),
            gas_limit: block.header.gas_limit(),
            prevrandao: block.header.mix_hash(),
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice::new(
                block.header.excess_blob_gas().unwrap_or_default(),
                false,
            )),
        };

        self
    }

    /// Infers the host from the provided url and adds it to the set of hosts
    pub fn with_url(mut self, url: &str) -> Self {
        let host = Url::parse(url)
            .ok()
            .and_then(|url| url.host().map(|host| host.to_string()))
            .unwrap_or(url.to_string());
        self.hosts.insert(host);
        self
    }

    /// Sets the [BlockEnv] of this instance
    pub fn set_block_env(mut self, block_env: revm::context::BlockEnv) {
        self.block_env = block_env;
    }
}

// ignore hosts to not invalidate the cache when different endpoints are used, as it's commonly the
// case for http vs ws endpoints
impl PartialEq for BlockchainDbMeta {
    fn eq(&self, other: &Self) -> bool {
        self.block_env == other.block_env
    }
}

impl<'de> Deserialize<'de> for BlockchainDbMeta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// A backwards compatible representation of [revm::primitives::BlockEnv]
        ///
        /// This prevents deserialization errors of cache files caused by breaking changes to the
        /// default [revm::primitives::BlockEnv], for example enabling an optional feature.
        /// By hand rolling deserialize impl we can prevent cache file issues
        struct BlockEnvBackwardsCompat {
            inner: revm::context::BlockEnv,
        }

        impl<'de> Deserialize<'de> for BlockEnvBackwardsCompat {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let mut value = serde_json::Value::deserialize(deserializer)?;

                // we check for any missing fields here
                if let Some(obj) = value.as_object_mut() {
                    let default_value =
                        serde_json::to_value(revm::context::BlockEnv::default()).unwrap();
                    for (key, value) in default_value.as_object().unwrap() {
                        if !obj.contains_key(key) {
                            obj.insert(key.to_string(), value.clone());
                        }
                    }
                }

                let cfg_env: revm::context::BlockEnv =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(Self { inner: cfg_env })
            }
        }

        // custom deserialize impl to not break existing cache files
        #[derive(Deserialize)]
        struct Meta {
            block_env: BlockEnvBackwardsCompat,
            /// all the hosts used to connect to
            #[serde(alias = "host")]
            hosts: Hosts,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Hosts {
            Multi(BTreeSet<String>),
            Single(String),
        }

        let Meta { block_env, hosts } = Meta::deserialize(deserializer)?;
        Ok(Self {
            block_env: block_env.inner,
            hosts: match hosts {
                Hosts::Multi(hosts) => hosts,
                Hosts::Single(host) => BTreeSet::from([host]),
            },
        })
    }
}

/// In Memory cache containing all fetched accounts and storage slots
/// and their values from RPC
#[derive(Debug, Default)]
pub struct MemDb {
    /// Account related data
    pub accounts: RwLock<AddressHashMap<AccountInfo>>,
    /// Storage related data
    pub storage: RwLock<AddressHashMap<StorageInfo>>,
    /// All retrieved block hashes
    pub block_hashes: RwLock<HashMap<U256, B256>>,
}

impl MemDb {
    /// Clears all data stored in this db
    pub fn clear(&self) {
        self.accounts.write().clear();
        self.storage.write().clear();
        self.block_hashes.write().clear();
    }

    // Inserts the account, replacing it if it exists already
    pub fn do_insert_account(&self, address: Address, account: AccountInfo) {
        self.accounts.write().insert(address, account);
    }

    /// The implementation of [DatabaseCommit::commit()]
    pub fn do_commit(&self, changes: HashMap<Address, Account>) {
        let mut storage = self.storage.write();
        let mut accounts = self.accounts.write();
        for (add, mut acc) in changes {
            if acc.is_empty() || acc.is_selfdestructed() {
                accounts.remove(&add);
                storage.remove(&add);
            } else {
                // insert account
                if let Some(code_hash) = acc
                    .info
                    .code
                    .as_ref()
                    .filter(|code| !code.is_empty())
                    .map(|code| code.hash_slow())
                {
                    acc.info.code_hash = code_hash;
                } else if acc.info.code_hash.is_zero() {
                    acc.info.code_hash = KECCAK_EMPTY;
                }
                accounts.insert(add, acc.info);

                let acc_storage = storage.entry(add).or_default();
                if acc.status.contains(AccountStatus::Created) {
                    acc_storage.clear();
                }
                for (index, value) in acc.storage {
                    if value.present_value().is_zero() {
                        acc_storage.remove(&index);
                    } else {
                        acc_storage.insert(index, value.present_value());
                    }
                }
                if acc_storage.is_empty() {
                    storage.remove(&add);
                }
            }
        }
    }
}

impl Clone for MemDb {
    fn clone(&self) -> Self {
        Self {
            storage: RwLock::new(self.storage.read().clone()),
            accounts: RwLock::new(self.accounts.read().clone()),
            block_hashes: RwLock::new(self.block_hashes.read().clone()),
        }
    }
}

impl DatabaseCommit for MemDb {
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        self.do_commit(changes)
    }
}

/// A DB that stores the cached content in a json file
#[derive(Debug)]
pub struct JsonBlockCacheDB {
    /// Where this cache file is stored.
    ///
    /// If this is a [None] then caching is disabled
    cache_path: Option<PathBuf>,
    /// Object that's stored in a json file
    data: JsonBlockCacheData,
}

impl JsonBlockCacheDB {
    /// Creates a new instance.
    fn new(meta: Arc<RwLock<BlockchainDbMeta>>, cache_path: Option<PathBuf>) -> Self {
        Self { cache_path, data: JsonBlockCacheData { meta, data: Arc::new(Default::default()) } }
    }

    /// Loads the contents of the diskmap file and returns the read object
    ///
    /// # Errors
    /// This will fail if
    ///   - the `path` does not exist
    ///   - the format does not match [JsonBlockCacheData]
    pub fn load(path: impl Into<PathBuf>) -> eyre::Result<Self> {
        let path = path.into();
        trace!(target: "cache", ?path, "reading json cache");
        let contents = std::fs::read_to_string(&path).map_err(|err| {
            warn!(?err, ?path, "Failed to read cache file");
            err
        })?;
        let data = serde_json::from_str(&contents).map_err(|err| {
            warn!(target: "cache", ?err, ?path, "Failed to deserialize cache data");
            err
        })?;
        Ok(Self { cache_path: Some(path), data })
    }

    /// Returns the [MemDb] it holds access to
    pub const fn db(&self) -> &Arc<MemDb> {
        &self.data.data
    }

    /// Metadata stored alongside the data
    pub const fn meta(&self) -> &Arc<RwLock<BlockchainDbMeta>> {
        &self.data.meta
    }

    /// Returns `true` if this is a transient cache and nothing will be flushed
    pub const fn is_transient(&self) -> bool {
        self.cache_path.is_none()
    }

    /// Flushes the DB to disk if caching is enabled.
    #[instrument(level = "warn", skip_all, fields(path = ?self.cache_path))]
    pub fn flush(&self) {
        let Some(path) = &self.cache_path else { return };
        self.flush_to(path.as_path());
    }

    /// Flushes the DB to a specific file
    pub fn flush_to(&self, cache_path: &Path) {
        let path: &Path = cache_path;

        trace!(target: "cache", "saving json cache");

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let file = match fs::File::create(path) {
            Ok(file) => file,
            Err(e) => return warn!(target: "cache", %e, "Failed to open json cache for writing"),
        };

        let mut writer = BufWriter::new(file);
        if let Err(e) = serde_json::to_writer(&mut writer, &self.data) {
            return warn!(target: "cache", %e, "Failed to write to json cache");
        }
        if let Err(e) = writer.flush() {
            return warn!(target: "cache", %e, "Failed to flush to json cache");
        }

        trace!(target: "cache", "saved json cache");
    }

    /// Returns the cache path.
    pub fn cache_path(&self) -> Option<&Path> {
        self.cache_path.as_deref()
    }
}

/// The Data the [JsonBlockCacheDB] can read and flush
///
/// This will be deserialized in a JSON object with the keys:
/// `["meta", "accounts", "storage", "block_hashes"]`
#[derive(Debug)]
pub struct JsonBlockCacheData {
    pub meta: Arc<RwLock<BlockchainDbMeta>>,
    pub data: Arc<MemDb>,
}

impl Serialize for JsonBlockCacheData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(4))?;

        map.serialize_entry("meta", &self.meta.read().clone())?;
        map.serialize_entry("accounts", &self.data.accounts.read().clone())?;
        map.serialize_entry("storage", &self.data.storage.read().clone())?;
        map.serialize_entry("block_hashes", &self.data.block_hashes.read().clone())?;

        map.end()
    }
}

impl<'de> Deserialize<'de> for JsonBlockCacheData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Data {
            meta: BlockchainDbMeta,
            accounts: AddressHashMap<AccountInfo>,
            storage: AddressHashMap<HashMap<U256, U256>>,
            block_hashes: HashMap<U256, B256>,
        }

        let Data { meta, accounts, storage, block_hashes } = Data::deserialize(deserializer)?;

        Ok(Self {
            meta: Arc::new(RwLock::new(meta)),
            data: Arc::new(MemDb {
                accounts: RwLock::new(accounts),
                storage: RwLock::new(storage),
                block_hashes: RwLock::new(block_hashes),
            }),
        })
    }
}

/// A type that flushes a `JsonBlockCacheDB` on drop
///
/// This type intentionally does not implement `Clone` since it's intended that there's only once
/// instance that will flush the cache.
#[derive(Debug)]
pub struct FlushJsonBlockCacheDB(pub Arc<JsonBlockCacheDB>);

impl Drop for FlushJsonBlockCacheDB {
    fn drop(&mut self) {
        trace!(target: "fork::cache", "flushing cache");
        self.0.flush();
        trace!(target: "fork::cache", "flushed cache");
    }
}
