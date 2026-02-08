# DB File Warmer for Reth — Exploration & Implementation Plan

## Problem

After a restart, the OS page cache is cold. MDBX reads hit disk for weeks until naturally warmed.
Nethermind solved this with `Db.StateDbEnableFileWarmer`, which reads all DB files sequentially at startup to prime the OS page cache.

## Current State in Reth

**No file warmer exists.** A search for "warm", "fadvise", "madvise", "page cache", "readahead" found only:
- The `no_rdahead: true` flag set during DB open (line 427 of `crates/storage/db/src/implementation/mdbx/mod.rs`) — this *disables* readahead for random access patterns
- MDBX's vendored C code has `mdbx_env_warmup()` API (discussed below)
- No Rust-side warmup logic exists anywhere

## Key Findings

### 1. MDBX Has a Built-in Warmup API

The vendored libmdbx already exposes `mdbx_env_warmup()` in `mdbx.h` (line 3278) and it's available in the auto-generated FFI bindings (`ffi::mdbx_env_warmup`). This is **not currently wrapped** in the Rust `reth-libmdbx` crate.

```c
// mdbx.h:3278
LIBMDBX_API int mdbx_env_warmup(
    const MDBX_env *env,
    const MDBX_txn *txn,
    MDBX_warmup_flags_t flags,
    unsigned timeout_seconds_16dot16  // fixed-point 16.16 format
);
```

Available flags (from FFI bindings):
| Flag | Value | Description |
|------|-------|-------------|
| `MDBX_warmup_default` | 0 | Ask OS kernel to async prefetch pages (uses `madvise(MADV_WILLNEED)`) |
| `MDBX_warmup_force` | 1 | Force-read all allocated pages sequentially into memory |
| `MDBX_warmup_oomsafe` | 2 | Use syscalls instead of direct access to avoid OOM-killer |
| `MDBX_warmup_lock` | 4 | Lock pages in memory via `mlock()` |
| `MDBX_warmup_touchlimit` | 8 | Auto-adjust resource limits for lock |
| `MDBX_warmup_release` | 16 | Release a previous lock |

**This is the preferred approach** — it's already built into MDBX, handles the mmap'd file correctly, and respects the database's internal geometry (only warms allocated pages, not the full file).

### 2. How Nethermind Does It (for reference)

Nethermind uses RocksDB (not MDBX), so their approach is different. From `DbOnTheRocks.cs`:

```csharp
private void WarmupFile(string basePath, RocksDb db)
{
    // Gets live SST file metadata from RocksDB
    // Sorts by creation time (oldest first)
    // Reads each file sequentially with 512KB buffer using Parallel.ForEach
    // Logs progress as percentage
    byte[] buffer = new byte[512.KiB()];
    using FileStream stream = File.OpenRead(fullPath);
    int readCount = buffer.Length;
    while (readCount == buffer.Length)
    {
        readCount = stream.Read(buffer);
        Interlocked.Add(ref totalRead, readCount);
    }
}
```

Key details:
- Enabled via `Db.StateDbEnableFileWarmer: true`
- Recommended for systems with ≥128GB RAM
- Reads files in parallel
- 512KB read buffer
- Logs progress as percentage

### 3. MDBX DB File Locations

MDBX stores two files in the DB directory:
- `mdbx.dat` — the main data file (defined as `MDBX_DATANAME "/mdbx.dat"` in mdbx.h:802)
- `mdbx.lck` — the lock file (defined as `MDBX_LOCKNAME "/mdbx.lck"` in mdbx.h:787)

The DB directory path is resolved via:
- `ChainPath::db()` → `<datadir>/<chain_id>/db` (crates/node/core/src/dirs.rs:288)
- Passed to `init_db(path, args)` (crates/storage/db/src/mdbx.rs:38)
- Then to `DatabaseEnv::open(path, kind, args)` (crates/storage/db/src/implementation/mdbx/mod.rs:348)

### 4. Database Opening Flow

```
NodeCommand::run()
  → init_db(db_path, args)           // crates/storage/db/src/mdbx.rs:38
    → create_db(path, args)          // crates/storage/db/src/mdbx.rs:17
      → DatabaseEnv::open(path, RW, args)  // crates/storage/db/src/implementation/mdbx/mod.rs:348
        → EnvironmentBuilder::open(path)   // crates/storage/libmdbx-rs/src/environment.rs:611
          → mdbx_env_create() + mdbx_env_open()
```

## Implementation Plan

### Approach: Use MDBX's Native `mdbx_env_warmup()` API

This is superior to manual file reading because:
1. It understands MDBX's internal page layout (only warms allocated pages)
2. Works correctly with MDBX's mmap (the data is already mmap'd — we just need to fault in pages)
3. Handles edge cases (OOM safety, timeouts)
4. Already implemented and tested in libmdbx
5. ~30 lines of Rust code total

### Step 1: Add `warmup()` to `Environment` in `reth-libmdbx`

**File:** `crates/storage/libmdbx-rs/src/environment.rs`

Add a method to `Environment`:

```rust
impl Environment {
    /// Warms up the database by loading pages into memory.
    ///
    /// Uses `mdbx_env_warmup()` to ask the OS to prefetch database pages,
    /// optionally force-loading them. This eliminates cold-start penalties
    /// after a restart.
    pub fn warmup(&self, flags: ffi::MDBX_warmup_flags_t, timeout_seconds: Option<u64>) -> Result<bool> {
        // timeout is in fixed-point 16.16 format: upper 16 bits = seconds
        let timeout = timeout_seconds.map(|s| (s as u32) << 16).unwrap_or(0);
        mdbx_result(unsafe {
            ffi::mdbx_env_warmup(
                self.env_ptr(),
                ptr::null(),  // no specific txn
                flags,
                timeout,
            )
        })
    }
}
```

### Step 2: Add `warmup_db` to `DatabaseEnv`

**File:** `crates/storage/db/src/implementation/mdbx/mod.rs`

```rust
impl DatabaseEnv {
    /// Spawns a background task to warm up the database by loading pages into memory.
    ///
    /// This uses MDBX's native `mdbx_env_warmup()` which asks the OS kernel to
    /// prefetch database pages into the page cache, eliminating cold-start penalties.
    pub fn warmup(&self) -> Result<(), DatabaseError> {
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("reth-db-warmup".to_string())
            .spawn(move || {
                let start = std::time::Instant::now();
                reth_tracing::tracing::info!(
                    target: "reth::db",
                    "Starting database warmup..."
                );

                // MDBX_warmup_default uses madvise(MADV_WILLNEED) for async prefetch
                // MDBX_warmup_force would synchronously touch every page
                let flags = ffi::MDBX_warmup_force | ffi::MDBX_warmup_oomsafe;
                match inner.warmup(flags, None) {
                    Ok(_) => {
                        reth_tracing::tracing::info!(
                            target: "reth::db",
                            elapsed = ?start.elapsed(),
                            "Database warmup complete"
                        );
                    }
                    Err(e) => {
                        reth_tracing::tracing::warn!(
                            target: "reth::db",
                            %e,
                            "Database warmup failed"
                        );
                    }
                }
            })
            .map_err(|e| DatabaseError::Other(e.to_string()))?;

        Ok(())
    }
}
```

### Step 3: Add `enable_db_warmup` to `DatabaseArguments`

**File:** `crates/storage/db/src/implementation/mdbx/mod.rs`

Add to `DatabaseArguments`:

```rust
pub struct DatabaseArguments {
    // ... existing fields ...
    /// Whether to warm up the database by loading pages into memory at startup.
    enable_warmup: bool,
}
```

With builder method:

```rust
impl DatabaseArguments {
    pub const fn with_warmup(mut self, enable: bool) -> Self {
        self.enable_warmup = enable;
        self
    }
}
```

### Step 4: Call warmup in `init_db`

**File:** `crates/storage/db/src/mdbx.rs`

After the database is opened in `init_db_for`, conditionally start warmup:

```rust
pub fn init_db_for<P: AsRef<Path>, TS: TableSet>(
    path: P,
    args: DatabaseArguments,
) -> eyre::Result<DatabaseEnv> {
    let enable_warmup = args.enable_warmup;
    let client_version = args.client_version().clone();
    let mut db = create_db(path, args)?;
    db.create_and_track_tables_for::<TS>()?;
    db.record_client_version(client_version)?;
    drop_orphan_tables(&db);

    if enable_warmup {
        if let Err(e) = db.warmup() {
            reth_tracing::tracing::warn!(
                target: "reth::db",
                %e,
                "Failed to start database warmup"
            );
        }
    }

    Ok(db)
}
```

### Step 5: Add CLI flag

Add `--db.warmup` flag to the database arguments, plumbed through the existing `DatabaseArgs` CLI struct.

**File:** `crates/node/core/src/args/database.rs` (or equivalent)

```rust
/// Enable database warmup at startup to prime the OS page cache.
#[arg(long = "db.warmup", default_value_t = false)]
pub warmup: bool,
```

## Alternative Approach: Manual File Reading

If we wanted to avoid touching the libmdbx wrapper, we could do what Nethermind does — manually read the `mdbx.dat` file:

```rust
fn warmup_file(path: PathBuf) {
    std::thread::Builder::new()
        .name("reth-db-warmup".to_string())
        .spawn(move || {
            let file_path = path.join("mdbx.dat");
            let file = match std::fs::File::open(&file_path) {
                Ok(f) => f,
                Err(e) => {
                    warn!(target: "reth::db", %e, "Failed to open mdbx.dat for warmup");
                    return;
                }
            };

            // Hint sequential access
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
                }
            }

            let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file); // 1MB buffer
            let mut buf = vec![0u8; 1024 * 1024];
            let mut total_read: u64 = 0;
            let start = std::time::Instant::now();

            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        total_read += n as u64;
                        if total_read % (1024 * 1024 * 1024) == 0 {
                            info!(
                                target: "reth::db",
                                progress = format!("{:.1}%", total_read as f64 / file_size as f64 * 100.0),
                                elapsed = ?start.elapsed(),
                                "Database warmup progress"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(target: "reth::db", %e, "Database warmup read error");
                        break;
                    }
                }
            }

            info!(
                target: "reth::db",
                elapsed = ?start.elapsed(),
                size_gb = total_read / (1024 * 1024 * 1024),
                "Database warmup complete"
            );
        })
        .ok();
}
```

**However, the MDBX native approach is strongly preferred** because MDBX uses mmap — the data file is already memory-mapped, and `mdbx_env_warmup` correctly faults in only the allocated pages through the existing mapping, which is what the page cache actually needs.

## Recommendation

**Use the MDBX native `mdbx_env_warmup()` API.** It's:
- Already available in the FFI bindings (auto-generated from mdbx.h)
- Handles mmap'd files correctly (touches pages through the mapping)
- OOM-safe with `MDBX_warmup_oomsafe` flag
- Supports timeouts
- Ignores lock files and only warms the data file
- ~30 lines of new Rust code across 3 files
- Zero new dependencies

The recommended flags for production:
- `MDBX_warmup_force | MDBX_warmup_oomsafe` — synchronously touches all pages, OOM-safe
- For async prefetch only (lighter): `MDBX_warmup_default` (just calls `madvise(MADV_WILLNEED)`)

## Files to Modify

| File | Change |
|------|--------|
| `crates/storage/libmdbx-rs/src/environment.rs` | Add `warmup()` method to `Environment` |
| `crates/storage/db/src/implementation/mdbx/mod.rs` | Add `warmup()` to `DatabaseEnv`, add `enable_warmup` to `DatabaseArguments` |
| `crates/storage/db/src/mdbx.rs` | Call warmup in `init_db_for` |
| `crates/node/core/src/args/database.rs` | Add `--db.warmup` CLI flag |

## Risks

- **Very low risk.** The warmup runs in a background thread and doesn't affect DB operations.
- If warmup fails, it logs a warning and the node continues normally.
- OOM-safe flag prevents the process from being killed if memory is insufficient.
- The MDBX `mdbx_env_warmup` function is mature — used by mdbx_dump, mdbx_chk, and mdbx_copy tools.
