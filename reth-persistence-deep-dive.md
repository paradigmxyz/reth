# Reth Persistence Layer Deep Dive

A comprehensive analysis of Reth's storage architecture, database implementation, and data flow patterns.

## Table of Contents

1. [Overview](#overview)
2. [High-Level Architecture](#high-level-architecture)
3. [Storage Layer Organization](#storage-layer-organization)
4. [Core Abstractions](#core-abstractions)
5. [Database Tables Schema](#database-tables-schema)
6. [MDBX Implementation](#mdbx-implementation)
7. [Static Files (NippyJar)](#static-files-nippyjar)
8. [Provider Layer](#provider-layer)
9. [Data Flow Examples](#data-flow-examples)
10. [Key Design Patterns](#key-design-patterns)
11. [The Pipeline: Why We Need Persistence](#the-pipeline-why-we-need-persistence)
12. [Engine API: The Bridge Between Consensus and Execution](#engine-api-the-bridge-between-consensus-and-execution)
13. [Reorgs and Unwinds: Detailed Mechanics](#reorgs-and-unwinds-detailed-mechanics)
14. [Cursor Deep Dive: The Intuitive Guide](#cursor-deep-dive-the-intuitive-guide)

---

## Overview

Reth implements a **hybrid storage architecture** combining:
- **MDBX**: A high-performance key-value store for dynamic, frequently updated data
- **Static Files (NippyJar)**: Immutable columnar storage for historical, append-only data

This dual approach optimizes for both:
- **Write performance**: MDBX handles state updates efficiently
- **Read performance**: Static files provide fast historical data access via memory-mapped I/O

---

## High-Level Architecture

```
                              +------------------------------------------+
                              |              Reth Node                    |
                              +------------------------------------------+
                                              |
                                              v
+----------------------------------------------------------------------------------------------------------+
|                                      PROVIDER LAYER                                                       |
|                                                                                                          |
|   +-------------------+    +---------------------+    +------------------------+    +-----------------+  |
|   | BlockchainProvider|    | DatabaseProvider    |    | StaticFileProvider     |    | StateProvider   |  |
|   |                   |    |                     |    |                        |    |                 |  |
|   | - Orchestrates    |    | - Read/Write ops    |    | - Immutable data       |    | - Current state |  |
|   | - Combines sources|    | - Transaction mgmt  |    | - Historical blocks    |    | - Historical    |  |
|   +-------------------+    +---------------------+    +------------------------+    +-----------------+  |
|            |                        |                           |                          |              |
+------------|------------------------|---------------------------|--------------------------|-------------+
             |                        |                           |                          |
             v                        v                           v                          v
+----------------------------------------------------------------------------------------------------------+
|                                      STORAGE API LAYER                                                    |
|                                                                                                          |
|   Traits: BlockReader, BlockWriter, AccountReader, StateReader, TrieReader, HeaderProvider...           |
|                                                                                                          |
+----------------------------------------------------------------------------------------------------------+
             |                                                    |
             v                                                    v
+---------------------------+                    +----------------------------------+
|        DB-API LAYER       |                    |         NIPPY-JAR LAYER          |
|                           |                    |                                  |
| Traits:                   |                    |  NippyJar<H>                     |
| - Database                |                    |  - Columnar storage              |
| - DbTx / DbTxMut          |                    |  - Compression support           |
| - DbCursorRO / DbCursorRW |                    |  - Memory-mapped reads           |
| - Table / DupSort         |                    |                                  |
+---------------------------+                    +----------------------------------+
             |                                                    |
             v                                                    v
+---------------------------+                    +----------------------------------+
|      MDBX IMPLEMENTATION  |                    |         STATIC FILES             |
|                           |                    |                                  |
|  DatabaseEnv              |                    |  *.idx  - Index files            |
|  - Tx<RO> / Tx<RW>        |                    |  *.off  - Offset files           |
|  - Cursor<K, T>           |                    |  *.conf - Configuration          |
|  - libmdbx FFI bindings   |                    |  data   - Compressed data        |
+---------------------------+                    +----------------------------------+
             |                                                    |
             v                                                    v
+---------------------------+                    +----------------------------------+
|     mdbx.dat (LMDB file)  |                    |    headers/, transactions/,      |
|                           |                    |    receipts/ directories         |
+---------------------------+                    +----------------------------------+
```

---

## Storage Layer Organization

The storage layer is organized into **11 interconnected crates** under `crates/storage/`:

```
crates/storage/
|
+-- db-api/              # Core database abstraction traits (database-agnostic)
|   +-- database.rs      # Database trait (entry point)
|   +-- transaction.rs   # DbTx, DbTxMut traits
|   +-- cursor.rs        # Cursor traits (DbCursorRO, DbCursorRW, DupCursors)
|   +-- table.rs         # Table trait, Encode/Decode, Compress/Decompress
|   +-- tables/mod.rs    # All 25+ table definitions
|
+-- db/                  # MDBX implementation
|   +-- implementation/
|   |   +-- mdbx/
|   |       +-- mod.rs   # DatabaseEnv (main database wrapper)
|   |       +-- tx.rs    # Tx<K> transaction implementation
|   |       +-- cursor.rs# Cursor<K, T> implementation
|   +-- metrics.rs       # Performance metrics
|   +-- lockfile.rs      # Concurrent access control
|
+-- storage-api/         # High-level provider traits
|   +-- block.rs         # BlockReader, BlockWriter
|   +-- state.rs         # StateReader, StateWriter
|   +-- trie.rs          # TrieReader, TrieWriter
|
+-- provider/            # Provider implementations
|   +-- providers/
|   |   +-- database/
|   |   |   +-- provider.rs       # DatabaseProvider<TX, N>
|   |   |   +-- blockchain_provider.rs
|   |   +-- static_file/
|   |       +-- manager.rs        # Static file management
|   +-- traits/
|       +-- full.rs              # FullProvider composite trait
|
+-- codecs/              # Serialization (Compact codec)
+-- nippy-jar/           # Immutable columnar storage format
+-- db-models/           # Data model wrapper types
+-- libmdbx-rs/          # Rust bindings for libmdbx C library
+-- errors/              # Error types
+-- zstd-compressors/    # Zstd compression
```

---

## Core Abstractions

### 1. Database Trait (`db-api/src/database.rs`)

The main entry point for database operations:

```rust
pub trait Database: Send + Sync + Debug {
    /// Read-only transaction type
    type TX: DbTx + Send + Sync + Debug + 'static;
    /// Read-write transaction type
    type TXMut: DbTxMut + DbTx + TableImporter + Send + Sync + Debug + 'static;

    /// Create read-only transaction
    fn tx(&self) -> Result<Self::TX, DatabaseError>;

    /// Create read-write transaction
    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError>;

    /// Execute closure with read-only transaction
    fn view<T, F>(&self, f: F) -> Result<T, DatabaseError>;

    /// Execute closure with read-write transaction
    fn update<T, F>(&self, f: F) -> Result<T, DatabaseError>;
}
```

### 2. Transaction Traits (`db-api/src/transaction.rs`)

```rust
/// Read-only transaction
pub trait DbTx: Debug + Send {
    type Cursor<T: Table>: DbCursorRO<T>;
    type DupCursor<T: DupSort>: DbDupCursorRO<T>;

    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError>;
    fn commit(self) -> Result<bool, DatabaseError>;
    fn abort(self);
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError>;
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError>;
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError>;
}

/// Read-write transaction
pub trait DbTxMut: Send {
    type CursorMut<T: Table>: DbCursorRW<T>;
    type DupCursorMut<T: DupSort>: DbDupCursorRW<T>;

    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
    fn append<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, DatabaseError>;
    fn clear<T: Table>(&self) -> Result<(), DatabaseError>;
    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError>;
}
```

### 3. Cursor Traits (`db-api/src/cursor.rs`)

```rust
/// Read-only cursor for iteration
pub trait DbCursorRO<T: Table> {
    fn first(&mut self) -> PairResult<T>;
    fn seek_exact(&mut self, key: T::Key) -> PairResult<T>;
    fn seek(&mut self, key: T::Key) -> PairResult<T>;  // >= key
    fn next(&mut self) -> PairResult<T>;
    fn prev(&mut self) -> PairResult<T>;
    fn last(&mut self) -> PairResult<T>;
    fn current(&mut self) -> PairResult<T>;
    fn walk(&mut self, start_key: Option<T::Key>) -> Result<Walker<T, Self>, DatabaseError>;
    fn walk_range(&mut self, range: impl RangeBounds<T::Key>) -> Result<RangeWalker<T, Self>, DatabaseError>;
    fn walk_back(&mut self, start_key: Option<T::Key>) -> Result<ReverseWalker<T, Self>, DatabaseError>;
}

/// Read-only cursor for duplicate-sorted tables
pub trait DbDupCursorRO<T: DupSort> {
    fn next_dup(&mut self) -> PairResult<T>;
    fn next_no_dup(&mut self) -> PairResult<T>;
    fn next_dup_val(&mut self) -> ValueOnlyResult<T>;
    fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T>;
    fn walk_dup(&mut self, key: Option<T::Key>, subkey: Option<T::SubKey>) -> Result<DupWalker<T, Self>, DatabaseError>;
}

/// Read-write cursor
pub trait DbCursorRW<T: Table> {
    fn upsert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError>;
    fn insert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError>;
    fn append(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError>;  // O(1) for sorted inserts
    fn delete_current(&mut self) -> Result<(), DatabaseError>;
}
```

### 4. Table Trait (`db-api/src/table.rs`)

```rust
pub trait Table: Send + Sync + Debug + 'static {
    const NAME: &'static str;
    const DUPSORT: bool;

    type Key: Encode + Decode + Ord;
    type Value: Compress + Decompress;
}

pub trait DupSort: Table {
    type SubKey: Encode + Decode;
}

/// For encoding keys (must be sortable)
pub trait Encode {
    type Encoded: AsRef<[u8]> + Into<Vec<u8>>;
    fn encode(self) -> Self::Encoded;
}

/// For compressing values
pub trait Compress {
    type Compressed: AsRef<[u8]>;
    fn compress(self) -> Self::Compressed;
}
```

---

## Database Tables Schema

### Block Data Tables

```
+-------------------------+-------------------+---------------------------+
|        Table            |       Key         |          Value            |
+-------------------------+-------------------+---------------------------+
| CanonicalHeaders        | BlockNumber       | HeaderHash (B256)         |
| Headers                 | BlockNumber       | Header                    |
| HeaderNumbers           | BlockHash (B256)  | BlockNumber               |
| BlockBodyIndices        | BlockNumber       | StoredBlockBodyIndices    |
| BlockOmmers             | BlockNumber       | StoredBlockOmmers         |
| BlockWithdrawals        | BlockNumber       | StoredBlockWithdrawals    |
+-------------------------+-------------------+---------------------------+
```

### Transaction Tables

```
+-------------------------+-------------------+---------------------------+
|        Table            |       Key         |          Value            |
+-------------------------+-------------------+---------------------------+
| Transactions            | TxNumber          | TransactionSigned         |
| TransactionHashNumbers  | TxHash (B256)     | TxNumber                  |
| TransactionBlocks       | TxNumber          | BlockNumber               |
| TransactionSenders      | TxNumber          | Address                   |
| Receipts                | TxNumber          | Receipt                   |
+-------------------------+-------------------+---------------------------+
```

### State Tables (Current State)

```
+-------------------------+-------------------+---------------------------+-----------+
|        Table            |       Key         |          Value            | DupSort   |
+-------------------------+-------------------+---------------------------+-----------+
| PlainAccountState       | Address           | Account                   | No        |
| PlainStorageState       | Address           | StorageEntry              | Yes (B256)|
| Bytecodes               | B256 (code hash)  | Bytecode                  | No        |
+-------------------------+-------------------+---------------------------+-----------+
```

### Historical State Tables

```
+-------------------------+------------------------+---------------------------+-----------+
|        Table            |       Key              |          Value            | DupSort   |
+-------------------------+------------------------+---------------------------+-----------+
| AccountsHistory         | ShardedKey<Address>    | BlockNumberList           | No        |
| StoragesHistory         | StorageShardedKey      | BlockNumberList           | No        |
| AccountChangeSets       | BlockNumber            | AccountBeforeTx           | Yes (Addr)|
| StorageChangeSets       | BlockNumberAddress     | StorageEntry              | Yes (B256)|
+-------------------------+------------------------+---------------------------+-----------+
```

### Trie Tables (Merkle Patricia Trie)

```
+-------------------------+------------------------+---------------------------+-----------+
|        Table            |       Key              |          Value            | DupSort   |
+-------------------------+------------------------+---------------------------+-----------+
| HashedAccounts          | B256 (keccak addr)     | Account                   | No        |
| HashedStorages          | B256 (keccak addr)     | StorageEntry              | Yes (B256)|
| AccountsTrie            | StoredNibbles          | BranchNodeCompact         | No        |
| StoragesTrie            | B256                   | StorageTrieEntry          | Yes       |
+-------------------------+------------------------+---------------------------+-----------+
```

### Metadata Tables

```
+-------------------------+-------------------+---------------------------+
|        Table            |       Key         |          Value            |
+-------------------------+-------------------+---------------------------+
| StageCheckpoints        | StageId (String)  | StageCheckpoint           |
| PruneCheckpoints        | PruneSegment      | PruneCheckpoint           |
| VersionHistory          | u64 (timestamp)   | ClientVersion             |
| ChainState              | ChainStateKey     | BlockNumber               |
| Metadata                | String            | Vec<u8>                   |
+-------------------------+-------------------+---------------------------+
```

---

## MDBX Implementation

### DatabaseEnv (`db/src/implementation/mdbx/mod.rs`)

```rust
pub struct DatabaseEnv {
    /// Underlying libmdbx environment
    inner: Environment,

    /// Cached database handles (DBI) for performance
    dbis: Arc<HashMap<&'static str, ffi::MDBX_dbi>>,

    /// Optional metrics collection
    metrics: Option<Arc<DatabaseEnvMetrics>>,

    /// File lock for read-write access
    _lock_file: Option<StorageLock>,
}

impl Database for DatabaseEnv {
    type TX = Tx<RO>;     // Read-only transaction
    type TXMut = Tx<RW>;  // Read-write transaction

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        Tx::new(
            self.inner.begin_ro_txn()?,
            self.dbis.clone(),
            self.metrics.clone(),
        )
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        Tx::new(
            self.inner.begin_rw_txn()?,
            self.dbis.clone(),
            self.metrics.clone(),
        )
    }
}
```

### Transaction Flow

```
+-------------------+
| DatabaseEnv::open |
+-------------------+
         |
         v
+-------------------+     +-------------------+
|   db.tx()         |     |   db.tx_mut()     |
+-------------------+     +-------------------+
         |                         |
         v                         v
+-------------------+     +-------------------+
|   Tx<RO>          |     |   Tx<RW>          |
| (read-only)       |     | (read-write)      |
+-------------------+     +-------------------+
         |                         |
         v                         v
+---------------------------------------------------+
|                   Operations                       |
|                                                   |
|   tx.get::<Table>(key)                            |
|   tx.cursor_read::<Table>()                       |
|   tx.put::<Table>(key, value)     (RW only)       |
|   tx.delete::<Table>(key, value)  (RW only)       |
|   tx.cursor_write::<Table>()      (RW only)       |
+---------------------------------------------------+
         |
         v
+-------------------+
|   tx.commit()     |  <-- Atomic commit
+-------------------+
```

### Key Configuration

```rust
pub struct DatabaseArguments {
    geometry: Geometry<Range<usize>>,  // Default: 0..8TB, 4GB growth steps
    log_level: Option<LogLevel>,
    max_read_transaction_duration: Option<MaxReadTransactionDuration>,
    exclusive: Option<bool>,
    max_readers: Option<u64>,          // Default: 32,000 (max: 32,767)
    sync_mode: SyncMode,               // Durable or SafeNoSync
}
```

---

## Static Files (NippyJar)

### Overview

NippyJar provides **immutable, columnar storage** for historical data that never changes (finalized blocks, transactions, receipts).

```
+----------------------------------+
|           NippyJar<H>            |
+----------------------------------+
| version: usize                   |
| user_header: H                   |
| columns: usize                   |
| rows: usize                      |
| compressor: Option<Compressors>  |
| max_row_size: usize              |
| path: PathBuf                    |
+----------------------------------+
         |
         |  Files on disk:
         |
         +---> data.jar      (compressed columnar data)
         +---> data.jar.idx  (index file)
         +---> data.jar.off  (offsets file)
         +---> data.jar.conf (configuration)
```

### Data Organization

```
                    Column 0          Column 1          Column 2
                   (Headers)     (Transactions)      (Receipts)
                  +---------+     +---------+       +---------+
        Row 0     |  Data   |     |  Data   |       |  Data   |
                  +---------+     +---------+       +---------+
        Row 1     |  Data   |     |  Data   |       |  Data   |
                  +---------+     +---------+       +---------+
        Row 2     |  Data   |     |  Data   |       |  Data   |
                  +---------+     +---------+       +---------+
           .          .               .                 .
           .          .               .                 .
```

### Reading Process

```
1. Load offset file (memory-mapped)
2. Calculate offset for (row, column)
3. Read compressed data from data file
4. Decompress and return
```

---

## Provider Layer

### DatabaseProvider

The main provider combining MDBX and static files:

```rust
pub struct DatabaseProvider<TX, N: NodeTypes> {
    /// Database transaction (RO or RW)
    tx: TX,

    /// Chain specification
    chain_spec: Arc<N::ChainSpec>,

    /// Static file provider for historical data
    static_file_provider: StaticFileProvider<N::Primitives>,

    /// Prune modes configuration
    prune_modes: PruneModes,
}
```

### Provider Trait Hierarchy

```
                    FullProvider
                         |
    +--------------------+--------------------+
    |          |         |         |          |
BlockReader  State   Header    Trie      Receipts
    |       Provider  Provider Provider  Provider
    |          |         |         |          |
    +--------------------+--------------------+
                         |
                  DatabaseProvider
                         |
            +------------+------------+
            |                         |
         MDBX                   StaticFiles
     (current state)         (historical data)
```

---

## Data Flow Examples

### Reading a Block

```
Application
     |
     v
+-----------------+
| block(number)?  |
+-----------------+
     |
     v
+-----------------+
| Check if block  |
| is in static    |---> Yes ---> Read from NippyJar
| files           |              (memory-mapped)
+-----------------+
     |
     No
     v
+-----------------+
| Open DB read TX |
| tx = db.tx()    |
+-----------------+
     |
     v
+-----------------+
| Create cursors  |
| for each table  |
+-----------------+
     |
     +---> Headers: cursor.seek_exact(number)
     +---> BlockBodyIndices: cursor.seek_exact(number)
     +---> Transactions: cursor.walk_range(tx_start..tx_end)
     +---> Receipts: cursor.walk_range(tx_start..tx_end)
     |
     v
+-----------------+
| Decompress data |
| Assemble Block  |
+-----------------+
     |
     v
+-----------------+
| tx.commit()     |
| (free pages)    |
+-----------------+
     |
     v
  Return Block
```

### Writing a Block

```
Application
     |
     v
+-----------------+
| write_block()   |
+-----------------+
     |
     v
+-----------------+
| Open write TX   |
| tx = db.tx_mut()|
+-----------------+
     |
     v
+-----------------+
| Create write    |
| cursors         |
+-----------------+
     |
     +---> Headers: cursor.append(number, header.compress())
     +---> BlockBodyIndices: cursor.append(number, indices.compress())
     +---> Transactions: for each tx -> cursor.append(tx_num, tx.compress())
     +---> Receipts: for each receipt -> cursor.append(tx_num, receipt.compress())
     +---> PlainAccountState: cursor.upsert(addr, account.compress())
     +---> PlainStorageState: cursor.upsert(addr, entry.compress())
     +---> AccountChangeSets: cursor.append_dup(block_num, change)
     +---> StorageChangeSets: cursor.append_dup(key, change)
     |
     v
+-----------------+
| tx.commit()     |
| (atomic write)  |
+-----------------+
```

### Historical State Lookup

```
Query: account_at(address, block_number)
     |
     v
+---------------------------+
| Check current state       |
| PlainAccountState[address]|
+---------------------------+
     |
     v
+---------------------------+
| Find relevant shard       |
| AccountsHistory.seek(     |
|   ShardedKey(addr, block) |
| )                         |
+---------------------------+
     |
     v
+---------------------------+
| Get block list where      |
| account changed           |
| BlockNumberList           |
+---------------------------+
     |
     v
+---------------------------+
| Find latest change        |
| before target block       |
+---------------------------+
     |
     v
+---------------------------+
| Read from changeset       |
| AccountChangeSets[block]  |
| .seek_by_key_subkey(      |
|   block, address          |
| )                         |
+---------------------------+
     |
     v
  Return historical Account
```

---

## Key Design Patterns

### 1. Trait-Based Abstraction

All database operations are defined through traits, enabling:
- Database-agnostic code
- Easy testing with mock implementations
- Potential for alternative backends (RocksDB, etc.)

### 2. Transaction Semantics

- All operations occur within transactions
- Transactions are atomic (all-or-nothing commit)
- RAII-based cleanup (drop aborts uncommitted transactions)
- Two types: `DbTx` (read-only) and `DbTxMut` (read-write)

### 3. Cursor Pattern

Efficient iteration and range queries:
- Pre-sorted key access
- O(1) append for sequential inserts
- Walker/ReverseWalker for convenient iteration
- DupSort support for multi-value keys

### 4. Serialization Pipeline

```
Value --> Compress --> Encode --> Store
Store --> Decode --> Decompress --> Value
```

- Keys use `Encode` trait (must be sortable bytes)
- Values use `Compress` trait (Compact codec by default)

### 5. Sharded Keys for History

Historical data uses sharded keys to enable efficient range queries:

```
ShardedKey<Address> = (Address, max_block_in_shard)

Example shards for address 0x1234:
  (0x1234, 100)    -> blocks 0-100
  (0x1234, 200)    -> blocks 101-200
  (0x1234, u64::MAX) -> blocks 201-current
```

### 6. DupSort Tables

Multiple values per key, sorted by subkey:
- `PlainStorageState`: Address -> [StorageEntry] (sorted by storage key)
- `AccountChangeSets`: BlockNumber -> [AccountBeforeTx] (sorted by address)

---

## Performance Considerations

### MDBX Optimizations

1. **Write-ahead logging disabled**: Uses writemap mode
2. **No readahead**: Better for random access patterns
3. **Page coalescing**: Reduces fragmentation
4. **Large max readers**: 32,000 concurrent readers
5. **Large geometry**: 8TB max size, 4GB growth steps

### Static File Optimizations

1. **Memory-mapped I/O**: Zero-copy reads
2. **Columnar storage**: Efficient for specific column access
3. **Compression**: Zstd or LZ4 per column
4. **Immutable**: No locking needed for reads

### Access Patterns

| Operation | MDBX | Static Files |
|-----------|------|--------------|
| Random read | Good | Excellent |
| Sequential read | Good | Excellent |
| Write | Excellent | N/A (immutable) |
| Historical query | Good | Excellent |
| Current state | Excellent | N/A |

---

## File Locations

### MDBX Database
```
<datadir>/db/mdbx.dat
<datadir>/db/mdbx.lck
```

### Static Files
```
<datadir>/static_files/
  headers/
    segment_0_1000000.jar
    segment_0_1000000.jar.idx
    segment_0_1000000.jar.off
  transactions/
    ...
  receipts/
    ...
```

---

## Quick Reference

### Essential Types

| Type | Location | Purpose |
|------|----------|---------|
| `Database` | db-api/src/database.rs | Main entry point trait |
| `DbTx` | db-api/src/transaction.rs | Read-only transaction |
| `DbTxMut` | db-api/src/transaction.rs | Read-write transaction |
| `DbCursorRO` | db-api/src/cursor.rs | Read cursor |
| `DbCursorRW` | db-api/src/cursor.rs | Write cursor |
| `Table` | db-api/src/table.rs | Table definition |
| `DatabaseEnv` | db/src/implementation/mdbx/mod.rs | MDBX wrapper |
| `Tx<K>` | db/src/implementation/mdbx/tx.rs | Transaction impl |
| `DatabaseProvider` | provider/src/providers/database/provider.rs | Main provider |
| `NippyJar` | nippy-jar/src/lib.rs | Static file format |

### Common Operations

```rust
// Open database
let db = DatabaseEnv::open(path, DatabaseEnvKind::RW, args)?;
db.create_tables()?;

// Read operation
let tx = db.tx()?;
let header = tx.get::<Headers>(block_number)?;
tx.commit()?;

// Write operation
let tx = db.tx_mut()?;
tx.put::<Headers>(block_number, header)?;
tx.commit()?;

// Cursor iteration
let tx = db.tx()?;
let mut cursor = tx.cursor_read::<Headers>()?;
for result in cursor.walk(Some(start_block))? {
    let (block_num, header) = result?;
    // process...
}
```

---

## The Pipeline: Why We Need Persistence

The pipeline is the heart of reth's synchronization system. Understanding **why persistence is critical** requires understanding how the pipeline works.

### What is the Pipeline?

The pipeline is a **staged synchronization architecture** that downloads and processes blockchain data in sequential, well-defined stages. Each stage handles a specific responsibility:

```
+--------------------------------------------------------------------------------+
|                              RETH PIPELINE                                      |
+--------------------------------------------------------------------------------+
|                                                                                |
|   +--------+    +--------+    +-----------+    +--------+    +--------+        |
|   |Headers |===>| Bodies |===>| Execution |===>| Hashing|===>| Merkle |===>... |
|   | Stage  |    | Stage  |    |   Stage   |    | Stages |    | Stage  |        |
|   +--------+    +--------+    +-----------+    +--------+    +--------+        |
|       |             |              |              |              |              |
|       v             v              v              v              v              |
|   +--------+    +--------+    +-----------+    +--------+    +--------+        |
|   |  Save  |    |  Save  |    |   Save    |    |  Save  |    |  Save  |        |
|   |Checkpoint   |Checkpoint   | Checkpoint|    |Checkpoint   |Checkpoint       |
|   +--------+    +--------+    +-----------+    +--------+    +--------+        |
|                                                                                |
+--------------------------------------------------------------------------------+
                                     |
                                     v
                        +------------------------+
                        |    MDBX Database       |
                        | (Atomic Persistence)   |
                        +------------------------+
```

### The Pipeline Stages (In Order)

```rust
pub enum StageId {
    Headers,              // 1. Download block headers from network
    Bodies,               // 2. Download block bodies (transactions)
    SenderRecovery,       // 3. Recover sender addresses from signatures
    Execution,            // 4. Execute transactions, update state
    AccountHashing,       // 5. Hash accounts for state root calculation
    StorageHashing,       // 6. Hash storage for state root calculation
    MerkleExecute,        // 7. Calculate Merkle state root
    TransactionLookup,    // 8. Build tx hash -> block number index
    IndexAccountHistory,  // 9. Index account change history
    IndexStorageHistory,  // 10. Index storage change history
    Prune,                // 11. Prune old data per configuration
    Finish,               // 12. Mark sync complete
}
```

### Why Persistence is Critical

#### 1. **Crash Recovery & Resumability**

```
Scenario: Node crashes during Execution stage at block 15,000,000

WITHOUT Persistence:
  - Restart from block 0
  - Re-download 15M headers (days of work)
  - Re-download 15M bodies
  - Re-execute 15M blocks (weeks of work)
  - Total: WEEKS of wasted work

WITH Persistence (Checkpoints):
  - Read checkpoint: Execution stage at block 14,999,500
  - Resume execution from block 14,999,501
  - Total: Minutes of work lost
```

#### 2. **Atomic Stage Commits**

Each stage commits atomically to the database:

```rust
// Inside execute_stage_to_completion()
match self.stage(stage_index).execute(&provider_rw, exec_input) {
    Ok(out @ ExecOutput { checkpoint, done }) => {
        // 1. Save checkpoint to database
        provider_rw.save_stage_checkpoint(stage_id, checkpoint)?;

        // 2. Commit ALL changes atomically
        provider_rw.commit()?;  // <-- ATOMIC: All or nothing

        // 3. Post-commit hooks (e.g., notify ExEx)
        self.stage(stage_index).post_execute_commit()?;
    }
    Err(err) => {
        // Transaction automatically rolled back (RAII)
        // No partial state corruption
    }
}
```

#### 3. **Unwind Support (Reorg Handling)**

When a chain reorganization is detected:

```
Block 100 (canonical) --> Block 101A (our chain)
                     \--> Block 101B (new canonical, more work)

Pipeline must:
1. UNWIND stages in REVERSE order back to block 100
2. RE-EXECUTE forward on new canonical chain

This requires:
- ChangeSets to reverse state changes
- Atomic unwinding (all or nothing)
- Checkpoint updates at each step
```

```rust
// Unwind process (stages/api/src/pipeline/mod.rs:300-411)
pub fn unwind(&mut self, to: BlockNumber, bad_block: Option<BlockNumber>) {
    // Unwind stages in REVERSE order
    let unwind_pipeline = self.stages.iter_mut().rev();

    for stage in unwind_pipeline {
        while checkpoint.block_number > to {
            let output = stage.unwind(&provider_rw, input);

            // Save new (lower) checkpoint
            provider_rw.save_stage_checkpoint(stage_id, checkpoint)?;
            provider_rw.commit()?;
        }
    }
}
```

### The Stage Trait: Execute and Unwind

Every stage must implement both forward execution AND backward unwinding:

```rust
pub trait Stage<Provider>: Send {
    fn id(&self) -> StageId;

    /// Execute stage forward (sync new blocks)
    fn execute(
        &mut self,
        provider: &Provider,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind stage backward (handle reorg)
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
```

### Execution Stage Deep Dive

The Execution stage is the most complex - it runs the EVM:

```rust
// stages/stages/src/stages/execution.rs

fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
    let start_block = input.next_block();
    let max_block = input.target();

    // Create EVM executor with current state
    let db = StateProviderDatabase(LatestStateProviderRef::new(provider));
    let mut executor = self.evm_config.batch_executor(db);

    for block_number in start_block..=max_block {
        // 1. Fetch block from database
        let block = provider.recovered_block(block_number)?;

        // 2. Execute all transactions in EVM
        let result = executor.execute_one(&block)?;

        // 3. Validate post-execution (consensus rules)
        self.consensus.validate_block_post_execution(&block, &result)?;

        // 4. Check batch thresholds
        if self.thresholds.is_end_of_batch(...) {
            break;  // Commit now, continue in next iteration
        }
    }

    // 5. Write state changes to database
    let state = ExecutionOutcome::from_blocks(start_block, executor.into_state());
    provider.write_state(&state, OriginalValuesKnown::Yes)?;

    Ok(ExecOutput { checkpoint, done })
}
```

### Tables Written by Each Stage

| Stage | Tables Written |
|-------|----------------|
| Headers | `CanonicalHeaders`, `Headers`, `HeaderNumbers` (+ Static Files) |
| Bodies | `BlockBodyIndices`, `Transactions` (+ Static Files) |
| SenderRecovery | `TransactionSenders` |
| Execution | `PlainAccountState`, `PlainStorageState`, `Bytecodes`, `AccountChangeSets`, `StorageChangeSets`, `Receipts` |
| AccountHashing | `HashedAccounts` |
| StorageHashing | `HashedStorages` |
| MerkleExecute | `AccountsTrie`, `StoragesTrie` |
| TransactionLookup | `TransactionHashNumbers`, `TransactionBlocks` |
| IndexAccountHistory | `AccountsHistory` |
| IndexStorageHistory | `StoragesHistory` |

### Batching and Thresholds

The Execution stage uses thresholds to batch commits:

```rust
pub struct ExecutionStageThresholds {
    pub max_blocks: Option<u64>,      // Max blocks before commit
    pub max_changes: Option<u64>,     // Max state changes before commit
    pub max_cumulative_gas: Option<u64>, // Max gas before commit
    pub max_duration: Option<Duration>,  // Max time before commit
}
```

**Why batch?**
- Fewer commits = less I/O overhead
- But larger batches = more work lost on crash
- Thresholds balance these trade-offs

### Data Flow: Pipeline to Persistence

```
                     PIPELINE EXECUTION
                           |
     +---------------------|---------------------+
     |                     v                     |
     |   +----------------------------------------+
     |   |           Stage Execution              |
     |   |                                        |
     |   |  1. Read from DB (current state)       |
     |   |  2. Process data (download/execute)    |
     |   |  3. Write to DB (new state + changes)  |
     |   |  4. Save checkpoint                    |
     |   +----------------------------------------+
     |                     |
     |                     v
     |   +----------------------------------------+
     |   |      provider_rw.commit()              |
     |   |                                        |
     |   |  - MDBX transaction commit             |
     |   |  - All table writes become durable     |
     |   |  - Checkpoint saved atomically         |
     |   +----------------------------------------+
     |                     |
     +---------------------|---------------------+
                           v
              +------------------------+
              |     move_to_static_    |
              |        files()         |
              +------------------------+
                           |
              +------------+------------+
              |                         |
              v                         v
     +----------------+        +----------------+
     | StaticFile     |        |    Pruner      |
     | Producer       |        |                |
     | (copy to jars) |        | (delete from   |
     +----------------+        |  MDBX)         |
                               +----------------+
```

### Things to Be Aware Of

#### 1. **Checkpoint Consistency**

The checkpoint represents the **last fully completed block**. A stage's checkpoint means:
- All blocks up to and including that number are processed
- The stage should resume from checkpoint + 1

```rust
impl ExecInput {
    pub fn next_block(&self) -> BlockNumber {
        self.checkpoint().block_number + 1  // Resume from next block
    }
}
```

#### 2. **Static File Consistency**

Data can exist in both MDBX and static files temporarily. The pipeline must:
1. Copy data from MDBX to static files
2. Prune data from MDBX
3. Handle crashes between these steps

```rust
// Consistency check in Execution stage
fn ensure_consistency(&self, provider: &Provider, checkpoint: u64) {
    // Compare database state vs static file state
    let db_receipt_num = provider.block_body_indices(checkpoint)?.next_tx_num();
    let static_file_receipt_num = static_file_provider
        .get_highest_static_file_tx(StaticFileSegment::Receipts);

    // If mismatch, fix it before proceeding
    match static_file_block_num.cmp(&checkpoint) {
        Ordering::Greater => {
            // Static file ahead: prune static file
            static_file_producer.prune_receipts(...)?;
        }
        Ordering::Less => {
            // Database ahead: error, need to unwind
            return Err(StageError::MissingStaticFileData { ... });
        }
    }
}
```

#### 3. **Unwind Limitations**

You cannot unwind past pruned data:

```rust
// Before unwinding, verify target is not pruned
prune_modes.ensure_unwind_target_unpruned(latest_block, to, &checkpoints)?;
```

#### 4. **Stage Dependencies**

Stages have implicit dependencies:
- Bodies stage needs Headers stage complete
- Execution needs Bodies complete
- Hashing needs Execution complete
- Merkle needs Hashing complete

The pipeline enforces this by running stages in order.

#### 5. **Detached Head Handling**

If our chain tip doesn't connect to the network's chain:

```rust
StageError::DetachedHead { local_head, header, error }
```

The pipeline will attempt to unwind and find a common ancestor. Multiple attempts are tracked to prevent infinite loops.

### Summary: Why Persistence Matters

| Requirement | How Persistence Helps |
|-------------|----------------------|
| **Crash Recovery** | Checkpoints allow resuming from last completed work |
| **Atomicity** | MDBX transactions ensure all-or-nothing commits |
| **Reorg Handling** | ChangeSets enable reversing state changes |
| **Consistency** | Static file consistency checks prevent corruption |
| **Progress Tracking** | Stage checkpoints show sync progress |
| **Historical Queries** | History tables enable state at any block |

---

## Engine API: The Bridge Between Consensus and Execution

The Engine API is how the **Consensus Layer (CL)** communicates with the **Execution Layer (EL)**. Post-merge Ethereum separates block proposal (CL) from block execution (EL), and the Engine API is the bridge.

### Architecture Overview

```
+-------------------+                              +-------------------+
|   Consensus       |                              |   Execution       |
|     Layer         |                              |     Layer         |
|  (Beacon Node)    |                              |     (Reth)        |
+-------------------+                              +-------------------+
        |                                                   |
        |    engine_newPayloadV3(payload)                   |
        |-------------------------------------------------->|
        |                     PayloadStatus                 |
        |<--------------------------------------------------|
        |                                                   |
        |    engine_forkchoiceUpdatedV3(state, attrs?)      |
        |-------------------------------------------------->|
        |                  PayloadStatus + PayloadId?       |
        |<--------------------------------------------------|
        |                                                   |
        |    engine_getPayloadV3(payload_id)                |
        |-------------------------------------------------->|
        |                  ExecutionPayload                 |
        |<--------------------------------------------------|
        |                                                   |
+-------------------+                              +-------------------+
```

### Key Engine API Methods

| Method | Purpose |
|--------|---------|
| `engine_newPayloadVX` | Submit a new block payload for validation & execution |
| `engine_forkchoiceUpdatedVX` | Update the canonical chain head, trigger payload building |
| `engine_getPayloadVX` | Retrieve a built payload for proposal |

### Core Data Structures

```rust
// Forkchoice state from CL
pub struct ForkchoiceState {
    pub head_block_hash: B256,       // Current chain head
    pub safe_block_hash: B256,       // Safe block (2/3 attestations)
    pub finalized_block_hash: B256,  // Finalized block (irreversible)
}

// Response statuses
pub enum PayloadStatusEnum {
    Valid,                           // Block is valid
    Invalid { latest_valid_hash },   // Block or ancestor invalid
    Syncing,                         // Still syncing, can't validate yet
    Accepted,                        // Payload accepted (newPayload only)
}
```

### Engine Tree: In-Memory Block Forest

The `EngineApiTreeHandler` manages an in-memory tree of executed blocks:

```
                         Persisted Chain (on disk)
                                 |
                        Block 100 (canonical)
                                 |
               +-----------------+-----------------+
               |                                   |
          Block 101A                          Block 101B
          (in-memory)                         (in-memory)
               |                                   |
          Block 102A                          Block 102B
          (in-memory)                         (current head)
               |
          Block 103A
          (in-memory, fork)
```

```rust
// crates/engine/tree/src/tree/state.rs
pub struct TreeState<N: NodePrimitives> {
    /// All executed blocks by hash (includes all forks)
    blocks_by_hash: HashMap<B256, ExecutedBlock<N>>,

    /// Blocks grouped by number (multiple per number due to forks)
    blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlock<N>>>,

    /// Parent-to-children mapping for traversal
    parent_to_child: HashMap<B256, HashSet<B256>>,

    /// Currently tracked canonical head
    current_canonical_head: BlockNumHash,
}
```

### Forkchoice State Tracker

Tracks the three types of forkchoice states:

```rust
// crates/engine/primitives/src/forkchoice.rs
pub struct ForkchoiceStateTracker {
    /// Most recent forkchoice (can be invalid!)
    latest: Option<ReceivedForkchoiceState>,

    /// Last forkchoice requiring sync
    last_syncing: Option<ForkchoiceState>,

    /// Last confirmed valid forkchoice
    last_valid: Option<ForkchoiceState>,
}

pub enum ForkchoiceStatus {
    Valid,    // Forkchoice applied successfully
    Invalid,  // Head or ancestor is invalid
    Syncing,  // Missing blocks, need to sync
}
```

### Engine API Request Flow

#### 1. newPayload Processing

```
engine_newPayloadV3(ExecutionPayload)
           |
           v
+------------------------+
| Convert to SealedBlock |
| Validate block format  |
+------------------------+
           |
           v
+------------------------+
| Check if already       |
| processed → VALID      |
+------------------------+
           |
           v
+------------------------+     +------------------------+
| Parent available?      |--No→| Buffer block           |
| (in memory or disk)    |     | Return SYNCING         |
+------------------------+     +------------------------+
           |
          Yes
           v
+------------------------+
| Get parent state       |
| Execute block in EVM   |
+------------------------+
           |
           +------------+------------+
           |                         |
      Success                    Failure
           |                         |
           v                         v
+------------------------+  +------------------------+
| Insert into TreeState  |  | Mark as invalid        |
| Emit event             |  | Return INVALID         |
| Try connect buffered   |  | + latestValidHash      |
| Return VALID           |  +------------------------+
+------------------------+
```

#### 2. forkchoiceUpdated Processing

```
engine_forkchoiceUpdatedV3(ForkchoiceState, PayloadAttributes?)
           |
           v
+------------------------+
| Validate forkchoice    |
| - head != 0x00         |
| - No invalid ancestor  |
| - Backfill not active  |
+------------------------+
           |
           v
+------------------------+     +------------------------+
| Head already canonical?|--Yes→| Process payload attrs |
| (current_head == head) |     | Return VALID           |
+------------------------+     +------------------------+
           |
          No
           v
+------------------------+
| Apply chain update     |
| (may trigger reorg)    |
+------------------------+
           |
           +------------+------------+
           |                         |
     Head found              Head missing
           |                         |
           v                         v
+------------------------+  +------------------------+
| on_new_head()          |  | Request block download |
| Compute new chain      |  | Return SYNCING         |
+------------------------+  +------------------------+
           |
           v
+------------------------+
| Update canonical state |
| Queue persistence      |
| Process payload attrs  |
| Return VALID           |
+------------------------+
```

---

## Reorgs and Unwinds: Detailed Mechanics

### What Triggers a Reorg?

A reorg occurs when `forkchoiceUpdated` specifies a head that:
1. Is NOT a direct descendant of the current canonical head
2. Shares a common ancestor with the current chain

```
Before reorg:                     After reorg:

Block 100 (canonical)             Block 100 (canonical)
    |                                 |
Block 101 (canonical)             Block 101 (orphaned)
    |                                 |
Block 102 (canonical, head)       Block 102 (orphaned)
                                      |
Block 101B (sidechain) ------>    Block 101B (canonical)
    |                                 |
Block 102B (sidechain)            Block 102B (canonical, new head)
```

### Computing the Reorg: on_new_head()

The `on_new_head` function computes whether we have a simple extension or a reorg:

```rust
// crates/engine/tree/src/tree/mod.rs:662-751
fn on_new_head(&self, new_head: B256) -> ProviderResult<Option<NewCanonicalChain<N>>> {
    let new_head_block = self.state.tree_state.blocks_by_hash.get(&new_head)?;
    let mut new_chain = vec![new_head_block.clone()];
    let mut current_hash = new_head_block.parent_hash();

    // Walk back the new chain collecting blocks
    while current_number > current_canonical_number {
        let block = self.state.tree_state.executed_block_by_hash(current_hash)?;
        current_hash = block.parent_hash();
        new_chain.push(block);
    }

    // Check if this is a simple extension
    if current_hash == self.current_canonical_head.hash {
        new_chain.reverse();
        return Ok(Some(NewCanonicalChain::Commit { new: new_chain }))
    }

    // We have a reorg - find the fork point
    let mut old_chain = Vec::new();
    let mut old_hash = self.current_canonical_head.hash;

    // Walk back old chain to same height
    while current_canonical_number > current_number {
        let block = self.canonical_block_by_hash(old_hash)?;
        old_hash = block.parent_hash();
        old_chain.push(block);
    }

    // Walk both chains until we find common ancestor
    while old_hash != current_hash {
        // Add block from old chain
        old_chain.push(self.canonical_block_by_hash(old_hash)?);
        old_hash = old_chain.last().unwrap().parent_hash();

        // Add block from new chain
        new_chain.push(self.state.tree_state.executed_block_by_hash(current_hash)?);
        current_hash = new_chain.last().unwrap().parent_hash();
    }

    new_chain.reverse();
    old_chain.reverse();

    Ok(Some(NewCanonicalChain::Reorg { new: new_chain, old: old_chain }))
}
```

### NewCanonicalChain Types

```rust
// crates/chain-state/src/in_memory.rs:893-907
pub enum NewCanonicalChain<N: NodePrimitives> {
    /// Simple extension - new blocks append to canonical head
    Commit {
        new: Vec<ExecutedBlock<N>>,  // Blocks to add
    },

    /// Reorg - need to replace old chain with new chain
    Reorg {
        new: Vec<ExecutedBlock<N>>,  // New canonical blocks
        old: Vec<ExecutedBlock<N>>,  // Orphaned blocks to remove
    },
}
```

### Reorg Detection Algorithm (Visual)

```
              Current State                    New Head Request

Canonical:    100 → 101 → 102                 FCU: head = 102B
                     |
Sidechain:          101B → 102B

Step 1: Walk back new_chain from 102B
        new_chain = [102B, 101B]
        current_hash = 100 (parent of 101B)

Step 2: current_hash (100) == canonical_head.hash (102)? NO

Step 3: Walk back old_chain to same height as new_chain tip
        old_chain = [102]
        old_hash = 101

Step 4: Walk both until hashes match (fork point)
        Iteration 1:
          old_chain += [101], old_hash = 100
          new_chain already at 101B's parent = 100
          current_hash = 100, old_hash = 100 → MATCH!

Result: Reorg {
    new: [101B, 102B],  // Blocks to apply
    old: [101, 102],    // Blocks to orphan
}
```

### Pipeline Unwind Process

When reorg affects persisted blocks, the pipeline must unwind:

```rust
// crates/stages/api/src/pipeline/mod.rs
pub fn unwind(&mut self, to: BlockNumber, bad_block: Option<BlockNumber>) {
    // Unwind stages in REVERSE order
    let unwind_pipeline = self.stages.iter_mut().rev();

    for stage in unwind_pipeline {
        let checkpoint = provider.get_stage_checkpoint(stage.id())?;

        // Skip if already at or below target
        if checkpoint.block_number <= to {
            continue;
        }

        // Unwind in batches
        while checkpoint.block_number > to {
            let input = UnwindInput {
                checkpoint,
                unwind_to: to,
                bad_block,
            };

            let output = stage.unwind(&provider_rw, input)?;

            // Save new (lower) checkpoint
            provider_rw.save_stage_checkpoint(stage.id(), output.checkpoint)?;
            provider_rw.commit()?;  // Atomic

            checkpoint = output.checkpoint;
        }
    }
}
```

### Execution Stage Unwind

The most complex unwind - must reverse state changes:

```rust
// Simplified from stages/stages/src/stages/execution.rs
fn unwind(&mut self, provider: &Provider, input: UnwindInput) -> Result<UnwindOutput, StageError> {
    let target = input.unwind_to;

    // 1. Get range of blocks to unwind
    let range = (target + 1)..=input.checkpoint.block_number;

    // 2. Read change sets for these blocks
    let account_changes = provider.account_changesets(range.clone())?;
    let storage_changes = provider.storage_changesets(range)?;

    // 3. Reverse the changes (restore old values)
    for (block_num, changes) in account_changes.into_iter().rev() {
        for AccountBeforeTx { address, info } in changes {
            match info {
                Some(old_account) => {
                    // Restore previous account state
                    provider.put::<PlainAccountState>(address, old_account)?;
                }
                None => {
                    // Account didn't exist before - delete it
                    provider.delete::<PlainAccountState>(address, None)?;
                }
            }
        }
    }

    // 4. Similar for storage changes...

    // 5. Delete receipts for unwound blocks
    provider.unwind_table_by_num::<Receipts>(target)?;

    // 6. Update checkpoint
    Ok(UnwindOutput { checkpoint: StageCheckpoint::new(target) })
}
```

### Why ChangeSets Are Critical

ChangeSets record the **before** values for every state change:

```
Block 102 Execution:
  - Account 0x1234: balance 100 → 50
  - Storage 0x1234[slot5]: 0 → 42

ChangeSet Stored:
  AccountChangeSet[102] = [(0x1234, Account{balance: 100})]  // Old value
  StorageChangeSet[102] = [(0x1234, slot5, 0)]               // Old value

To Unwind Block 102:
  1. Read ChangeSet[102]
  2. Restore 0x1234.balance = 100
  3. Restore 0x1234[slot5] = 0
  4. Delete receipt for block 102
```

### Dual Sync Modes

The Engine API uses two sync strategies:

```
+-------------------+                    +-------------------+
|    Live Sync      |                    |  Backfill Sync    |
| (BlockDownloader) |                    |    (Pipeline)     |
+-------------------+                    +-------------------+
|                   |                    |                   |
| For small gaps    |                    | For large gaps    |
| (< EPOCH_SLOTS)   |                    | (≥ EPOCH_SLOTS)   |
|                   |                    |                   |
| Download + execute|                    | Full staged sync  |
| blocks one by one |                    | (parallel stages) |
|                   |                    |                   |
| Quick response to |                    | Efficient for     |
| CL forkchoice     |                    | catching up       |
+-------------------+                    +-------------------+

Threshold: MIN_BLOCKS_FOR_PIPELINE_RUN = EPOCH_SLOTS (32 blocks)
```

### Persistence Service: Background Writing

The Engine API doesn't block on persistence:

```rust
// crates/engine/tree/src/persistence.rs
pub struct PersistenceService<N> {
    provider: ProviderFactory<N>,
    incoming: Receiver<PersistenceAction<N>>,
    pruner: PrunerWithFactory<...>,
}

pub enum PersistenceAction<N> {
    SaveBlocks(Vec<ExecutedBlock<N>>, oneshot::Sender<...>),
    RemoveBlocksAbove(u64, oneshot::Sender<...>),  // For reorgs
    SaveFinalizedBlock(u64),
    SaveSafeBlock(u64),
}

// Flow:
// 1. Engine handler executes blocks in-memory
// 2. Sends PersistenceAction::SaveBlocks to background service
// 3. Handler continues processing new requests
// 4. Background service writes to DB, triggers pruner
```

```
+------------------+          +------------------+
|  Engine Handler  |          | Persistence Svc  |
|  (main thread)   |          | (background)     |
+------------------+          +------------------+
        |                              |
        | SaveBlocks([101, 102])       |
        |----------------------------->|
        |                              |
        | (continues processing)       | Write to static files
        | newPayload(103)              | Write to MDBX
        |                              | Run pruner
        |                              |
        |<----- Done notification -----|
        |                              |
```

### Block Buffer: Handling Orphans

When a block's parent is missing, it's buffered:

```rust
// crates/engine/tree/src/tree/block_buffer.rs
pub struct BlockBuffer<B: Block> {
    blocks: HashMap<B256, SealedBlock<B>>,
    parent_to_children: HashMap<B256, HashSet<B256>>,
    limit: u32,  // Max buffered blocks
}

// When parent arrives:
fn try_connect_buffered_blocks(&mut self, parent_hash: B256) {
    if let Some(children) = self.parent_to_children.get(&parent_hash) {
        for child_hash in children.clone() {
            let child = self.blocks.remove(&child_hash)?;
            // Execute child block
            self.insert_payload(child)?;
            // Recursively connect grandchildren
            self.try_connect_buffered_blocks(child_hash);
        }
    }
}
```

### State Provider Building

Creating state for block execution with in-memory overlay:

```rust
// crates/engine/tree/src/tree/mod.rs
pub struct StateProviderBuilder<N, P> {
    provider_factory: P,
    historical: B256,               // Base block on disk
    overlay: Option<Vec<ExecutedBlock<N>>>,  // In-memory blocks
}

impl StateProviderBuilder {
    pub fn build(&self) -> ProviderResult<StateProviderBox> {
        // 1. Get historical state from disk
        let provider = self.provider_factory.state_by_block_hash(self.historical)?;

        // 2. Apply in-memory block changes on top
        if let Some(overlay) = self.overlay.clone() {
            Ok(Box::new(MemoryOverlayStateProvider::new(provider, overlay)))
        } else {
            Ok(provider)
        }
    }
}
```

### Complete Data Flow: forkchoiceUpdated with Reorg

```
[Consensus Layer]
        |
        v
engine_forkchoiceUpdatedV3(head=102B, safe=100, finalized=99)
        |
        v
+-------------------------------------------+
| EngineApiTreeHandler::on_forkchoice_updated|
+-------------------------------------------+
        |
        v
+-------------------------------------------+
| validate_forkchoice_state()               |
| - head != 0x00 ✓                          |
| - No invalid ancestors ✓                  |
| - Backfill idle ✓                         |
+-------------------------------------------+
        |
        v
+-------------------------------------------+
| head (102B) != current_head (102) → reorg |
| apply_chain_update()                      |
+-------------------------------------------+
        |
        v
+-------------------------------------------+
| on_new_head(102B)                         |
| Returns: Reorg {                          |
|   new: [101B, 102B],                      |
|   old: [101, 102]                         |
| }                                         |
+-------------------------------------------+
        |
        v
+-------------------------------------------+
| on_canonical_chain_update()               |
| 1. Update TreeState.canonical_head        |
| 2. Update CanonicalInMemoryState          |
| 3. Emit CanonicalChainCommitted event     |
| 4. Send to PersistenceService             |
+-------------------------------------------+
        |
        +----------------+
        |                |
        v                v
+----------------+  +-------------------+
| Return VALID   |  | Background:       |
| to CL          |  | - Save 101B, 102B |
+----------------+  | - Prune 101, 102  |
                    +-------------------+
        |
        v
[Notification to RPC/ExEx listeners]
CanonStateNotification::Reorg { new, old }
```

### Things to Be Aware Of (Engine API)

#### 1. **In-Memory vs Persisted Reorgs**

- **In-memory reorg**: Just update `TreeState.canonical_head`
- **Persisted reorg**: Requires pipeline unwind + re-execute

```rust
// Determine if persistence is needed
if reorg_affects_persisted_blocks {
    // Must unwind persisted state
    pipeline.unwind(fork_point)?;
    // Then re-execute on new chain
    for block in new_chain {
        execute_and_persist(block)?;
    }
} else {
    // Just update in-memory canonical pointer
    tree_state.set_canonical_head(new_head);
}
```

#### 2. **Finalization Cleans Sidechains**

When a block is finalized, all competing chains below it are pruned:

```rust
// crates/engine/tree/src/tree/state.rs:197-247
fn prune_finalized_sidechains(&mut self, finalized: BlockNumHash) {
    // Remove all blocks below finalized number
    // Keep only the finalized block at its height
    // BFS remove all children of non-finalized blocks
}
```

#### 3. **Invalid Block Caching**

Invalid blocks are cached to prevent reprocessing:

```rust
pub struct InvalidHeaderCache {
    invalid: HashMap<B256, BlockNumHash>,
}

// Check before processing
if invalid_cache.contains(block_hash) {
    return PayloadStatus::Invalid {
        latest_valid_hash: invalid_cache.get_latest_valid(block_hash)
    }
}
```

#### 4. **Sync Target Tracking**

The forkchoice tracker remembers where we need to sync to:

```rust
// If we return SYNCING, we track the target
if status.is_syncing() {
    self.last_syncing = Some(forkchoice_state);
}

// Later, download can query sync target
let target = forkchoice_tracker.sync_target();
```

---

## Cursor Deep Dive: The Intuitive Guide

Cursors are fundamental to how reth interacts with MDBX. This section explains cursors at both intuitive and technical levels.

### The Phonebook Analogy

Think of a cursor like **your finger on a sorted phonebook**:

```
Database Table (sorted by key):
┌─────────────────────────────────┐
│ Adams, John    → 555-0101       │
│ Brown, Alice   → 555-0102       │  ← cursor position (your finger)
│ Clark, Bob     → 555-0103       │
│ Davis, Eve     → 555-0104       │
│ ...                             │
└─────────────────────────────────┘
```

**Basic cursor operations map to finger movements:**

| Operation | What it does | Phonebook analogy |
|-----------|--------------|-------------------|
| `seek("Clark")` | Jump to >= "Clark" | Flip to the C section |
| `seek_exact("Clark")` | Jump to exactly "Clark" | Find "Clark, Bob" exactly |
| `next()` | Move to next entry | Slide finger down one row |
| `prev()` | Move to previous entry | Slide finger up one row |
| `first()` | Jump to beginning | Go to page 1 |
| `last()` | Jump to end | Go to last page |
| `current()` | Read without moving | Read where finger is |

### Why Cursors Instead of get()?

You might wonder: why not just use `tx.get(key)` for everything?

```
Scenario: Read blocks 1000 to 1010

Using get() - 11 separate lookups:
  get(1000) → B+ tree traversal → find leaf → read
  get(1001) → B+ tree traversal → find leaf → read  (same leaf!)
  get(1002) → B+ tree traversal → find leaf → read  (same leaf!)
  ...
  Total: 11 × O(log n) tree traversals

Using cursor - 1 lookup + 10 nexts:
  seek(1000) → B+ tree traversal → find leaf → read
  next() → already on leaf, move to next slot → read
  next() → already on leaf, move to next slot → read
  ...
  Total: 1 × O(log n) + 10 × O(1)
```

**Cursors exploit data locality.** Once you find a leaf node, sequential reads are nearly free.

### DupSort Tables: Multiple Values Per Key

Some tables store **multiple values per key**. Think of it as:

```
Regular Table (PlainAccountState):
  Address A → Account{balance: 100}
  Address B → Account{balance: 200}

DupSort Table (PlainStorageState):
  Address A → [
    StorageEntry{slot: 0, value: 42},
    StorageEntry{slot: 1, value: 100},
    StorageEntry{slot: 5, value: 0}
  ]
  Address B → [
    StorageEntry{slot: 0, value: 7}
  ]
```

**Phonebook analogy for DupSort:**
Like a person with multiple phone numbers, sorted:
```
Adams, John:
  - Home:   555-0101
  - Mobile: 555-0102
  - Work:   555-0103
Brown, Alice:
  - Mobile: 555-0201
```

**DupSort cursor operations:**

| Operation | What it does |
|-----------|--------------|
| `next_dup()` | Next value for same key (John's next number) |
| `next_no_dup()` | Skip to next key entirely (jump to Alice) |
| `seek_by_key_subkey(addr, slot)` | Find specific storage slot for address |
| `walk_dup(key, subkey)` | Iterate all slots for one address |

### Trie Cursors: Walking a Flattened Tree

The Merkle Patricia Trie is stored **flattened** in the database. Trie cursors help traverse it:

```
Conceptual Tree:              Flattened in DB (sorted by path):
        root                  ┌──────────────────────────────────┐
       /    \                 │ ""      → BranchNode{...}        │
     0x1    0x2               │ "1"     → BranchNode{...}        │
     /        \               │ "1a"    → BranchNode{hash: 0x..} │
   0x1a      0x2b             │ "2"     → BranchNode{...}        │
                              │ "2b"    → BranchNode{hash: 0x..} │
                              └──────────────────────────────────┘
```

**Trie cursor operations:**

```rust
// Find node at exact path
cursor.seek_exact(Nibbles::from_hex("1a"))?;

// Find node at or after path
cursor.seek(Nibbles::from_hex("1"))?;  // Finds "1" or "1a" or "2"...

// Iterate through trie in order
while let Some((path, node)) = cursor.next()? {
    // Process node
}
```

### The DupSort Upsert Gotcha

**CRITICAL:** For DupSort tables, `upsert()` doesn't replace - it appends!

```rust
// WRONG - Creates duplicates!
cursor.upsert(address, StorageEntry { slot: 5, value: 100 })?;
cursor.upsert(address, StorageEntry { slot: 5, value: 200 })?;

// Result: TWO entries for slot 5!
// Address A → [slot:5→100, slot:5→200]  // CORRUPTED

// CORRECT - Delete first, then insert
cursor.seek_by_key_subkey(address, slot)?;
cursor.delete_current()?;
cursor.upsert(address, StorageEntry { slot: 5, value: 200 })?;

// Result: Single correct entry
// Address A → [slot:5→200]
```

Why? Because DupSort keys aren't unique - the database can't know which duplicate you want to update.

### Walker Patterns

Walkers wrap cursors in Rust iterators:

```rust
// Manual cursor loop
let mut cursor = tx.cursor_read::<Headers>()?;
cursor.seek(1000)?;
loop {
    match cursor.next()? {
        Some((num, header)) if num <= 2000 => { /* process */ }
        _ => break,
    }
}

// Using Walker (cleaner)
let mut cursor = tx.cursor_read::<Headers>()?;
for result in cursor.walk_range(1000..=2000)? {
    let (num, header) = result?;
    // process
}

// Using DupWalker (for DupSort tables)
let mut cursor = tx.cursor_dup_read::<PlainStorageState>()?;
for result in cursor.walk_dup(Some(address), None)? {
    let (addr, entry) = result?;
    // process all storage slots for address
}
```

### Performance: append() vs insert()

When writing sorted data, `append()` is O(1) vs `insert()`'s O(log n):

```rust
// SLOW - Each insert searches the B+ tree
for i in 0..1_000_000 {
    cursor.insert(i, value)?;  // O(log n) each
}

// FAST - Append knows data is sorted, skips search
for i in 0..1_000_000 {
    cursor.append(i, value)?;  // O(1) each
}
```

**append() requires:** Data must be inserted in sorted order. If you append out of order, you'll corrupt the database!

### Real-World Usage Patterns

#### Pattern 1: Reading a Range (Execution Stage)

```rust
// Read all transactions in a block
let indices = provider.block_body_indices(block_num)?;
let mut cursor = tx.cursor_read::<Transactions>()?;

for result in cursor.walk_range(indices.first_tx_num..indices.first_tx_num + indices.tx_count)? {
    let (tx_num, transaction) = result?;
    // Execute transaction
}
```

#### Pattern 2: Writing State Changes

```rust
// Write account updates from execution
let mut cursor = tx.cursor_write::<PlainAccountState>()?;

for (address, account) in state_changes {
    cursor.upsert(address, account)?;
}
```

#### Pattern 3: Reversing State (Unwind)

```rust
// Read change sets to undo state
let mut changeset_cursor = tx.cursor_read::<AccountChangeSets>()?;
let mut state_cursor = tx.cursor_write::<PlainAccountState>()?;

for result in changeset_cursor.walk_dup(Some(block_num), None)?.rev() {
    let (_, AccountBeforeTx { address, info }) = result?;

    match info {
        Some(old_account) => state_cursor.upsert(address, old_account)?,
        None => { state_cursor.seek_exact(address)?; state_cursor.delete_current()?; }
    }
}
```

#### Pattern 4: Trie Traversal (State Root Calculation)

```rust
// Iterate account trie nodes
let mut cursor = tx.cursor_read::<AccountsTrie>()?;

for result in cursor.walk(None)? {
    let (nibbles, node) = result?;

    if node.state_mask.is_empty() && node.hash_mask.is_empty() {
        // Leaf node
    } else {
        // Branch node
    }
}
```

### Summary: When to Use What

| Scenario | Use This |
|----------|----------|
| Single key lookup | `tx.get::<Table>(key)` |
| Range of keys | `cursor.walk_range(start..end)` |
| All entries | `cursor.walk(None)` |
| Multiple values per key | `cursor.walk_dup(key, subkey)` |
| Sequential writes (sorted) | `cursor.append(key, value)` |
| Random writes | `cursor.upsert(key, value)` |
| Update DupSort entry | `delete_current()` then `upsert()` |
| Reverse iteration | `cursor.walk_back(start)` |

---

*Generated from reth source code analysis - December 2024*
