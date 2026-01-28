use std::hash::{Hash, Hasher};

use parking_lot::RwLock;
use smallvec::SmallVec;

use crate::Database;

/// Cached database entry.
///
/// Uses hash-only comparison since 64-bit hash collisions are negligible
/// for practical database counts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CachedDb {
    /// Hash of database name (None hashes distinctly from any string).
    name_hash: u64,
    /// The cached database (dbi + flags).
    db: Database,
}

impl CachedDb {
    /// Creates a new cached database entry.
    pub(crate) fn new(name: Option<&str>, db: Database) -> Self {
        let name_hash = Self::hash_name(name);
        Self { name_hash, db }
    }

    #[inline]
    pub(crate) fn hash_name(name: Option<&str>) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        name.hash(&mut hasher);
        hasher.finish()
    }
}

impl From<CachedDb> for Database {
    fn from(value: CachedDb) -> Self {
        value.db
    }
}

/// Simple cache container for database handles.
///
/// Uses inline storage for the common case (most apps use < 16 databases).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub(crate) struct DbCache(SmallVec<[CachedDb; 16]>);

impl DbCache {
    /// Read a database entry from the cache.
    pub(crate) fn read_db(&self, name_hash: u64) -> Option<Database> {
        for entry in self.0.iter() {
            if entry.name_hash == name_hash {
                return Some(entry.db);
            }
        }
        None
    }

    /// Write a database entry to the cache.
    pub(crate) fn write_db(&mut self, db: CachedDb) {
        for entry in self.0.iter() {
            if entry.name_hash == db.name_hash {
                return; // Another thread beat us
            }
        }
        self.0.push(db);
    }

    /// Remove a database entry from the cache by dbi.
    pub(crate) fn remove_dbi(&mut self, dbi: ffi::MDBX_dbi) {
        self.0.retain(|entry| entry.db.dbi() != dbi);
    }
}

/// Simple cache container for database handles.
///
/// Uses inline storage for the common case (most apps use < 16 databases).
#[derive(Debug)]
pub(crate) struct SharedCache {
    cache: RwLock<DbCache>,
}

impl SharedCache {
    /// Creates a new empty cache.
    fn new() -> Self {
        Self { cache: RwLock::new(DbCache::default()) }
    }

    /// Returns a read guard to the cache.
    fn read(&self) -> parking_lot::RwLockReadGuard<'_, DbCache> {
        self.cache.read()
    }

    /// Returns a write guard to the cache.
    fn write(&self) -> parking_lot::RwLockWriteGuard<'_, DbCache> {
        self.cache.write()
    }

    /// Read a database entry from the cache.
    pub(crate) fn read_db(&self, name_hash: u64) -> Option<Database> {
        let cache = self.read();
        cache.read_db(name_hash)
    }

    /// Write a database entry to the cache.
    pub(crate) fn write_db(&self, db: CachedDb) {
        let mut cache = self.write();
        cache.write_db(db);
    }

    /// Remove a database entry from the cache by dbi.
    pub(crate) fn remove_dbi(&self, dbi: ffi::MDBX_dbi) {
        let mut cache = self.write();
        cache.remove_dbi(dbi);
    }
}

impl Default for SharedCache {
    fn default() -> Self {
        Self::new()
    }
}
