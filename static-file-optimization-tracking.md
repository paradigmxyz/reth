# Tracking: Static File DB Write & Read Optimization

## Problem Statement

Static file DB operations are a critical bottleneck for high-throughput chains like Gravity that require sub-second block generation. Currently, block persistence occurs in the background with non-persistent blocks stored in `CanonicalInMemoryState`. When write speed is insufficient, blocks accumulate in memory, ultimately leading to OOM.

**Critical Performance Issue**: With 10 million accounts, a block containing 5000 transactions has a persistence time of up to **5 seconds**. For sub-second block generation, this creates an ever-growing memory backlog.

**Root Cause**: Sequential processing patterns, lack of resource reuse (DB transactions, cursors, memory allocations), excessive synchronous I/O, and missed parallelization opportunities throughout the static file write and read paths.

## Motivation

Generally we should reuse DB transactions, cursors, and memory allocations as much as possible, removing unnecessary channels, spawns, and sequential bottlenecks. The static file implementation has sound architecture but conservative implementation that leaves significant performance on the table.

## Current Architecture Issues

### Write Path Cascade
```
StaticFileProducer::run()
  → segments.par_iter() (✅ Parallel - GOOD)
    → Segment::copy_to_static_files() (❌ Sequential block processing)
      → for block in block_range (❌ Sequential)
        → provider.block_body_indices(block) (❌ DB lookup per block)
        → increment_block() (❌ Can trigger fsync mid-write)
        → cursor.walk_range() (❌ Cursor per block)
        → append_transaction/receipt (❌ Sequential)
  → commit() (❌ 3 sync_all() calls - blocks ~9-45ms)
  → update_index() (❌ Acquires write locks, blocks readers)
```

### Read Path Issues
```
fetch_range_with_predicate()
  → get_segment_provider() (❌ DashMap lookup + possible disk I/O)
    → cursor.get_one() (❌ Decompress on every read, no caching)
      → File boundary transition (❌ Must drop & recreate provider/cursor)
```

## Impact

- **Write latency**: 5 seconds for 5000-tx block (target: <1 second for Gravity)
- **Memory pressure**: Blocks accumulate in `CanonicalInMemoryState` faster than persistence
- **OOM risk**: High transaction throughput chains with 10M+ accounts
- **Read latency**: Redundant decompression, provider recreation overhead

---

## Sub-Issue 1: Sequential Block Processing Bottleneck

**Location**:
- `crates/static-file/static-file/src/segments/transactions.rs:27-59`
- `crates/static-file/static-file/src/segments/receipts.rs:27-56`

**Current Implementation**:
```rust
for block in block_range {
    static_file_writer.increment_block(block)?;

    // DB lookup per block - SEQUENTIAL
    let block_body_indices = provider
        .block_body_indices(block)?
        .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

    // Cursor per block
    let mut cursor = provider.tx_ref().cursor_read::<tables::Transactions>()?;
    let transactions_walker = cursor.walk_range(block_body_indices.tx_num_range())?;

    // Sequential iteration
    for entry in transactions_walker {
        let (tx_number, transaction) = entry?;
        static_file_writer.append_transaction(tx_number, &transaction)?;
    }
}
```

**Problems**:
1. Database lookups happen sequentially for each block (`block_body_indices`)
2. Transaction cursor recreated for every block
3. No prefetching of next block's data while processing current block
4. `increment_block()` can trigger file rotation (commit + fsync) during the write loop

**Performance Impact**:
- For 1000 blocks: 1000 DB lookups + 1000 increment operations
- Average 100 transactions per block: 100,000 sequential append calls
- Each operation blocks on previous operation completion

**Proposed Solution**:
Implement parallel block preparation pipeline:

```rust
fn copy_to_static_files_parallel(
    &self,
    provider: Provider,
    block_range: RangeInclusive<BlockNumber>,
) -> ProviderResult<()> {
    const PREFETCH_WINDOW: usize = 10;

    // Phase 1: Parallel prefetch pipeline
    let (prepared_tx, prepared_rx) = mpsc::sync_channel(PREFETCH_WINDOW);

    rayon::spawn(move || {
        block_range.par_bridge().try_for_each(|block| {
            // Parallel DB reads
            let block_body_indices = provider.block_body_indices(block)?;

            // Fetch all transactions for this block
            let transactions: Vec<_> = /* fetch with reused cursor */;

            prepared_tx.send((block, transactions))?;
            Ok(())
        })
    });

    // Phase 2: Sequential write (required for append-only file)
    while let Ok((block, transactions)) = prepared_rx.recv() {
        static_file_writer.increment_block(block)?;
        for (tx_num, tx) in transactions {
            static_file_writer.append_transaction(tx_num, &tx)?;
        }
    }

    Ok(())
}
```

**Expected Improvement**: 2-3x write throughput (from 5s to 1.5-2s for 5000-tx block)

**Implementation Effort**: Medium (2-3 weeks)

**Priority**: 🔥 CRITICAL

---

## Sub-Issue 2: Synchronous fsync Blocks Write Pipeline

**Location**:
- `crates/storage/nippy-jar/src/writer.rs:344-354` (commit)
- `crates/storage/nippy-jar/src/writer.rs:375-380` (commit_offsets)
- `crates/storage/provider/src/providers/static_file/writer.rs:235-274` (commit)

**Current Implementation**:
```rust
pub fn commit(&mut self) -> Result<(), NippyJarError> {
    self.data_file.flush()?;
    self.data_file.get_ref().sync_all()?;     // ← BLOCKS 1-5ms

    self.commit_offsets()?;                    // ← BLOCKS 1-5ms (contains sync_all)
    self.jar.freeze_config()?;                 // ← BLOCKS 1-5ms (contains sync_all)
    self.dirty = false;

    Ok(())
}
```

**Problems**:
1. Three separate `sync_all()` operations per commit (data file, offsets file, config file)
2. Each fsync blocks for 1-5ms on typical SSDs
3. Total blocking time: 3-15ms per segment commit
4. With 3 segments (headers/transactions/receipts): 9-45ms total per batch
5. Happens synchronously during write path

**Performance Impact**:
- 100 blocks/sec throughput = 100 commits = **900-4500ms wasted on fsync**
- Write thread idle during fsync operations
- No pipelining of commits

**Proposed Solution**:
Implement async fsync queue with commit pipelining:

```rust
struct AsyncCommitQueue {
    pending: mpsc::Sender<PendingCommit>,
    worker_handle: JoinHandle<()>,
}

struct PendingCommit {
    data_path: PathBuf,
    offsets_path: PathBuf,
    config_path: PathBuf,
    completion_signal: oneshot::Sender<Result<()>>,
}

impl AsyncCommitQueue {
    fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel(16);

        let worker_handle = std::thread::spawn(move || {
            while let Ok(commit) = pending_rx.recv() {
                // Batch multiple fsyncs if available
                let batch = drain_available(&pending_rx, commit);

                // Perform fsync operations in background
                for commit in batch {
                    let result = perform_fsync(&commit);
                    commit.completion_signal.send(result);
                }
            }
        });

        Self { pending: pending_tx, worker_handle }
    }
}

impl StaticFileProviderRW {
    pub fn commit_async(&mut self) -> ProviderResult<CommitHandle> {
        // Flush buffers (fast)
        self.data_file.flush()?;
        self.offsets_file.flush()?;

        // Queue fsync to background thread
        let (tx, rx) = oneshot::channel();
        self.async_queue.pending.send(PendingCommit { ... })?;

        Ok(CommitHandle { completion: rx })
    }
}
```

**Critical Consideration**:
- Block execution MUST ensure finalized blocks are fully persisted before continuing
- Solution: Await commit handles only for finalized blocks, allow speculative writes to proceed

**Expected Improvement**: 1.5-2x reduction in write latency (from 5s to 2.5-3.3s)

**Implementation Effort**: Medium (3-4 weeks)

**Priority**: 🔥 CRITICAL

---

## Sub-Issue 3: File Rotation Mid-Write Blocks Pipeline

**Location**: `crates/storage/provider/src/providers/static_file/writer.rs:335-372`

**Current Implementation**:
```rust
pub fn increment_block(&mut self, expected_block_number: BlockNumber) -> ProviderResult<()> {
    // ... validation ...

    if last_block == self.writer.user_header().expected_block_end() {
        // File is full, rotate - BLOCKS HERE
        self.commit()?;  // ← 3 sync_all() operations!

        // Open new file - more I/O
        let (writer, data_path) = Self::open(
            segment,
            last_block + 1,
            self.reader.clone(),
            self.metrics.clone()
        )?;

        self.writer = writer;
        self.data_path = data_path;

        // Create new segment header
        *self.writer.user_header_mut() = SegmentHeader::new(...);
    }

    self.writer.user_header_mut().increment_block();
    Ok(())
}
```

**Problems**:
1. File rotation happens **synchronously** during write path
2. Commits current file (3 fsync operations = 3-15ms)
3. Opens new file (file system operations)
4. All writes blocked during rotation
5. Unpredictable latency spikes when hitting 500K block boundary

**Performance Impact**:
- File rotation every 500K blocks
- Each rotation: 5-20ms blocked time
- For high-throughput chains: frequent rotations = frequent latency spikes

**Proposed Solution**:
Pre-allocate and prepare next file before rotation needed:

```rust
struct StaticFileProviderRW {
    writer: NippyJarWriter,
    next_writer: Option<NippyJarWriter>,  // Pre-prepared
    // ...
}

impl StaticFileProviderRW {
    fn maybe_prepare_next_file(&mut self) -> ProviderResult<()> {
        // When within 1000 blocks of file end, prepare next file
        if self.blocks_until_rotation() < 1000 && self.next_writer.is_none() {
            rayon::spawn(|| {
                let next_writer = Self::open(
                    segment,
                    next_file_start_block,
                    ...
                )?;
                self.next_writer = Some(next_writer);
            });
        }
        Ok(())
    }

    pub fn increment_block(&mut self, expected_block_number: BlockNumber) -> ProviderResult<()> {
        if last_block == self.writer.user_header().expected_block_end() {
            // Atomic swap - no blocking!
            self.commit_async()?;  // Background fsync

            std::mem::swap(
                &mut self.writer,
                self.next_writer.as_mut().expect("pre-prepared")
            );

            self.next_writer = None;
        }

        self.maybe_prepare_next_file()?;
        self.writer.user_header_mut().increment_block();
        Ok(())
    }
}
```

**Expected Improvement**: Eliminates 5-20ms latency spikes at file boundaries

**Implementation Effort**: Small (1 week)

**Priority**: 🔥 HIGH

---

## Sub-Issue 4: Index Updates Acquire Write Locks During Critical Path

**Location**:
- `crates/storage/provider/src/providers/static_file/writer.rs:307-329`
- `crates/storage/provider/src/providers/static_file/manager.rs:592-664`

**Current Implementation**:
```rust
fn update_index(&self) -> ProviderResult<()> {
    let segment_max_block = /* calculation */;
    self.reader().update_index(
        self.writer.user_header().segment(),
        segment_max_block
    )
}

// In manager.rs
pub fn update_index(
    &self,
    segment: StaticFileSegment,
    segment_max_block: Option<BlockNumber>,
) -> ProviderResult<()> {
    let mut max_block = self.static_files_max_block.write();  // ← BLOCKS READERS
    let mut tx_index = self.static_files_tx_index.write();    // ← BLOCKS READERS

    // Complex operations while holding locks
    match segment_max_block {
        Some(segment_max_block) => {
            max_block.insert(segment, segment_max_block);
            let fixed_range = self.find_fixed_range(segment_max_block);

            // Disk I/O while holding write lock!
            let jar = NippyJar::<SegmentHeader>::load(
                &self.path.join(segment.filename(&fixed_range)),
            ).map_err(ProviderError::other)?;

            // HashMap/BTreeMap operations
            if let Some(tx_range) = jar.user_header().tx_range() {
                // ... complex index updates ...
            }

            // DashMap operations
            self.map.insert((fixed_range.end(), segment), LoadedJar::new(jar)?);
            self.map.retain(|...| ...);
        }
        None => { /* ... */ }
    };

    Ok(())
}
```

**Problems**:
1. Acquires **two** write locks (`static_files_max_block`, `static_files_tx_index`)
2. Performs disk I/O while holding write locks (NippyJar::load)
3. Complex HashMap/BTreeMap/DashMap operations while readers blocked
4. Called after **every commit** (frequent lock contention)
5. No batching of multiple updates

**Performance Impact**:
- Every commit blocks all readers until index update completes
- Lock contention increases with concurrent read load
- Disk I/O amplifies blocking time (1-10ms)

**Proposed Solution**:
Batch index updates and defer to background thread:

```rust
struct PendingIndexUpdate {
    segment: StaticFileSegment,
    max_block: Option<BlockNumber>,
}

struct IndexUpdateQueue {
    pending: RwLock<Vec<PendingIndexUpdate>>,
    flush_tx: mpsc::Sender<()>,
}

impl IndexUpdateQueue {
    fn new(provider: Arc<StaticFileProviderInner>) -> Self {
        let (flush_tx, flush_rx) = mpsc::channel();

        std::thread::spawn(move || {
            while let Ok(()) = flush_rx.recv() {
                // Collect all pending updates
                let updates = std::mem::take(&mut *self.pending.write());

                // Apply all updates atomically
                provider.apply_index_updates_batch(updates)?;
            }
        });

        Self { pending: RwLock::new(Vec::new()), flush_tx }
    }

    fn queue_update(&self, segment: StaticFileSegment, max_block: Option<BlockNumber>) {
        self.pending.write().push(PendingIndexUpdate { segment, max_block });
    }

    fn flush_async(&self) {
        self.flush_tx.send(()).ok();
    }
}

impl StaticFileProviderInner {
    fn apply_index_updates_batch(&self, updates: Vec<PendingIndexUpdate>) -> ProviderResult<()> {
        // Acquire locks once for all updates
        let mut max_block = self.static_files_max_block.write();
        let mut tx_index = self.static_files_tx_index.write();

        // Pre-load all NippyJars (can be parallelized)
        let jars: HashMap<_, _> = updates.par_iter()
            .map(|update| {
                let jar = /* load jar */;
                (update.segment, jar)
            })
            .collect();

        // Fast updates with locks held
        for update in updates {
            let jar = &jars[&update.segment];
            // ... apply update ...
        }

        Ok(())
    }
}
```

**Expected Improvement**:
- Reduces lock contention by batching
- Eliminates disk I/O during write lock hold
- 1.1-1.3x improvement in concurrent read/write scenarios

**Implementation Effort**: Medium (2 weeks)

**Priority**: 🔥 HIGH

---

## Sub-Issue 5: Per-Block Receipt Iteration Negates Batching Benefits

**Location**: `crates/static-file/static-file/src/segments/receipts.rs:36-52`

**Current Implementation**:
```rust
for block in block_range {
    static_file_writer.increment_block(block)?;

    let block_body_indices = provider
        .block_body_indices(block)?
        .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

    let mut receipts_cursor = provider
        .tx_ref()
        .cursor_read::<tables::Receipts>()?;
    let receipts_walker = receipts_cursor.walk_range(block_body_indices.tx_num_range())?;

    // Batched append, BUT called per block!
    static_file_writer.append_receipts(
        receipts_walker.map(|result| result.map_err(ProviderError::from)),
    )?;
}
```

**Problems**:
1. `append_receipts()` is designed for batching (see `writer.rs:619-660`)
2. BUT it's called inside a per-block loop
3. Cursor recreated for every block
4. Function call overhead + iterator setup per block
5. Metrics recording per block instead of per batch

**Performance Impact**:
- For 1000 blocks: 1000 function calls instead of 1
- 1000 cursor creations
- 1000 iterator setups
- Negates most batching benefits

**Proposed Solution**:
Accumulate receipts across multiple blocks:

```rust
fn copy_to_static_files(
    &self,
    provider: Provider,
    block_range: RangeInclusive<BlockNumber>,
) -> ProviderResult<()> {
    let static_file_provider = provider.static_file_provider();
    let mut static_file_writer = static_file_provider
        .get_writer(*block_range.start(), StaticFileSegment::Receipts)?;

    const BATCH_SIZE: usize = 100;
    let mut receipt_batch = Vec::with_capacity(BATCH_SIZE * 100); // Assume ~100 tx/block

    // Reuse cursor across blocks
    let mut receipts_cursor = provider
        .tx_ref()
        .cursor_read::<tables::Receipts>()?;

    for (idx, block) in block_range.enumerate() {
        static_file_writer.increment_block(block)?;

        let block_body_indices = provider
            .block_body_indices(block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

        // Accumulate receipts
        let receipts_walker = receipts_cursor.walk_range(block_body_indices.tx_num_range())?;
        receipt_batch.extend(receipts_walker.map(|r| r.map_err(ProviderError::from)));

        // Flush batch periodically
        if (idx + 1) % BATCH_SIZE == 0 || block == *block_range.end() {
            static_file_writer.append_receipts(receipt_batch.drain(..))?;
        }
    }

    Ok(())
}
```

**Expected Improvement**: 1.2-1.5x for receipts segment write throughput

**Implementation Effort**: Small (1 week)

**Priority**: 🟡 MEDIUM

---

## Sub-Issue 6: No Decompression Caching on Read Path

**Location**: `crates/storage/db/src/static_file/cursor.rs:56-97`

**Current Implementation**:
```rust
pub fn get_one<M: ColumnSelectorOne>(
    &mut self,
    key_or_num: KeyOrNumber<'_>,
) -> ColumnResult<M::FIRST> {
    let row = self.get(key_or_num, M::MASK)?;

    match row {
        Some(row) => Ok(Some(M::FIRST::decompress(row[0])?)),  // ← ALWAYS decompress
        None => Ok(None),
    }
}
```

**Problems**:
1. Every read decompresses from scratch
2. Headers use LZ4 compression (~5-20μs per decompress)
3. Transactions/Receipts use zstd dictionary compression (~10-50μs per decompress)
4. No caching of recently accessed data
5. Redundant work for repeated queries (common in RPC scenarios)

**Performance Impact**:
- Reading 100 headers: 100 × 5-20μs = 0.5-2ms wasted
- Reading 1000 transactions: 1000 × 10-50μs = 10-50ms wasted
- RPC queries often access same blocks repeatedly (e.g., latest block)

**Proposed Solution**:
Add LRU decompression cache:

```rust
use lru::LruCache;

struct DecompressionCache {
    // Key: (segment, row_number, column_mask)
    cache: RwLock<LruCache<(StaticFileSegment, u64, usize), Vec<u8>>>,
    max_entries: usize,
    max_memory: usize,
    current_memory: AtomicUsize,
}

impl DecompressionCache {
    fn new(max_entries: usize, max_memory: usize) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(max_entries.try_into().unwrap())),
            max_entries,
            max_memory,
            current_memory: AtomicUsize::new(0),
        }
    }

    fn get_or_decompress<F>(
        &self,
        key: (StaticFileSegment, u64, usize),
        decompress_fn: F,
    ) -> Result<Vec<u8>, ProviderError>
    where
        F: FnOnce() -> Result<Vec<u8>, ProviderError>,
    {
        // Fast path: check read lock
        if let Some(cached) = self.cache.read().peek(&key) {
            return Ok(cached.clone());
        }

        // Slow path: decompress and cache
        let decompressed = decompress_fn()?;
        let size = decompressed.len();

        let mut cache = self.cache.write();

        // Evict if over memory limit
        while self.current_memory.load(Ordering::Relaxed) + size > self.max_memory {
            if let Some((_, evicted)) = cache.pop_lru() {
                self.current_memory.fetch_sub(evicted.len(), Ordering::Relaxed);
            } else {
                break;
            }
        }

        cache.put(key, decompressed.clone());
        self.current_memory.fetch_add(size, Ordering::Relaxed);

        Ok(decompressed)
    }
}

impl StaticFileCursor<'_> {
    pub fn get_one_cached<M: ColumnSelectorOne>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
        cache: &DecompressionCache,
    ) -> ColumnResult<M::FIRST> {
        let row_num = /* extract row number */;
        let cache_key = (self.segment(), row_num, M::MASK);

        let decompressed = cache.get_or_decompress(cache_key, || {
            let row = self.get(key_or_num, M::MASK)?.ok_or(/* error */)?;
            Ok(row[0].to_vec())
        })?;

        Ok(Some(M::FIRST::decompress(&decompressed)?))
    }
}
```

**Configuration**:
- Default: 10,000 entries, 100MB memory limit
- Adjustable via node config for different workloads

**Expected Improvement**:
- 2-5x speedup for repeated reads (hot data)
- 1.1-1.3x improvement for random reads (cache misses)

**Implementation Effort**: Medium (2 weeks)

**Priority**: 🟡 MEDIUM (HIGH for RPC-heavy nodes)

---

## Sub-Issue 7: Provider Recreation at File Boundaries

**Location**: `crates/storage/provider/src/providers/static_file/manager.rs:1095-1174`

**Current Implementation**:
```rust
pub fn fetch_range_with_predicate<T, F, P>(
    &self,
    segment: StaticFileSegment,
    range: Range<u64>,
    mut get_fn: F,
    mut predicate: P,
) -> ProviderResult<Vec<T>> {
    let mut provider = get_provider!(range.start);
    let mut cursor = provider.cursor()?;

    for number in range {
        'inner: loop {
            match get_fn(&mut cursor, number)? {
                Some(res) => { /* ... */ break 'inner },
                None => {
                    // Crossing file boundary
                    drop(cursor);              // ← Must drop
                    drop(provider);            // ← Must drop
                    provider = get_provider!(number);  // ← DashMap lookup
                    cursor = provider.cursor()?;       // ← Re-create cursor
                    retrying = true;
                }
            }
        }
    }

    Ok(result)
}
```

**Problems**:
1. File boundary crossings (every 500K blocks) require full provider/cursor teardown
2. DashMap lookup for next provider (hashing + lock acquisition)
3. Possible disk I/O if provider not cached
4. Memory mapping re-establishment for cursor
5. Comment indicates potential deadlock concern (lines 1159-1162)

**Performance Impact**:
- Reading 1M blocks: 2 file boundary crossings
- Each crossing: 50-200μs (DashMap + cursor recreation)
- Range queries frequently cross boundaries

**Proposed Solution**:
Prefetch next provider when approaching boundary:

```rust
struct RangeQueryState<'a, N> {
    current_provider: StaticFileJarProvider<'a, N>,
    current_cursor: StaticFileCursor<'a>,
    next_provider: Option<StaticFileJarProvider<'a, N>>,
    prefetch_threshold: u64,
}

impl<N: NodePrimitives> StaticFileProvider<N> {
    pub fn fetch_range_with_prefetch<T, F, P>(
        &self,
        segment: StaticFileSegment,
        range: Range<u64>,
        get_fn: F,
        predicate: P,
    ) -> ProviderResult<Vec<T>> {
        let mut state = RangeQueryState {
            current_provider: get_provider!(range.start),
            current_cursor: /* ... */,
            next_provider: None,
            prefetch_threshold: 1000,
        };

        for number in range {
            // Prefetch next provider when close to boundary
            if state.should_prefetch(number) && state.next_provider.is_none() {
                let next_range = self.find_fixed_range(state.current_file_end() + 1);

                // Spawn prefetch in background
                rayon::spawn(move || {
                    state.next_provider = Some(
                        self.get_or_create_jar_provider(segment, &next_range).ok()
                    );
                });
            }

            match get_fn(&mut state.current_cursor, number)? {
                Some(res) => {
                    if !predicate(&res) { break; }
                    result.push(res);
                }
                None => {
                    // Transition to next file
                    if let Some(next_provider) = state.next_provider.take() {
                        // Fast swap - no blocking!
                        state.current_provider = next_provider;
                        state.current_cursor = state.current_provider.cursor()?;
                    } else {
                        // Fallback to synchronous load
                        state.current_provider = get_provider!(number);
                        state.current_cursor = state.current_provider.cursor()?;
                    }
                }
            }
        }

        Ok(result)
    }
}
```

**Expected Improvement**: 1.2-1.5x for cross-file range queries

**Implementation Effort**: Medium (2 weeks)

**Priority**: 🟡 MEDIUM

---

## Sub-Issue 8: Transaction Hash Lookup Requires Linear File Search

**Location**: `crates/storage/provider/src/providers/static_file/manager.rs:1074-1093`

**Current Implementation**:
```rust
pub fn find_static_file<T>(
    &self,
    segment: StaticFileSegment,
    func: impl Fn(StaticFileJarProvider<'_, N>) -> ProviderResult<Option<T>>,
) -> ProviderResult<Option<T>> {
    if let Some(highest_block) = self.get_highest_static_file_block(segment) {
        let mut range = self.find_fixed_range(highest_block);

        // Linear search through files (reverse order)
        while range.end() > 0 {
            if let Some(res) = func(self.get_or_create_jar_provider(segment, &range)?)? {
                return Ok(Some(res))
            }

            // Move to previous file
            range = SegmentRangeInclusive::new(
                range.start().saturating_sub(self.blocks_per_file),
                range.end().saturating_sub(self.blocks_per_file),
            );
        }
    }

    Ok(None)
}
```

**Usage**: `transaction_by_hash`, `receipt_by_hash`

**Problems**:
1. Linear search through all static files (starts from most recent)
2. Each file requires: provider load + full file scan
3. No bloom filters for quick rejection
4. No hash indices for O(1) lookup
5. Worst case: scan all 20+ files (for 10M+ blocks)

**Performance Impact**:
- 10M blocks = 20 static files
- Worst case lookup: 20 file opens + 20 full scans
- Average case (assuming uniform distribution): 10 files
- Each file scan: 1-10ms depending on size

**Proposed Solution Option 1**: Bloom filters per static file

```rust
struct StaticFileBloomFilter {
    filter: BloomFilter,
    file_range: SegmentRangeInclusive,
}

impl StaticFileProviderInner {
    fn build_bloom_filters(&self, segment: StaticFileSegment) -> Vec<StaticFileBloomFilter> {
        // Build bloom filter for each static file
        // Store in separate .bloom files alongside .dat files
    }

    pub fn find_with_bloom<T>(
        &self,
        segment: StaticFileSegment,
        hash: &TxHash,
        func: impl Fn(StaticFileJarProvider<'_, N>) -> ProviderResult<Option<T>>,
    ) -> ProviderResult<Option<T>> {
        let bloom_filters = self.load_bloom_filters(segment)?;

        for bloom in bloom_filters.iter().rev() {
            // Quick rejection
            if !bloom.filter.contains(hash) {
                continue;
            }

            // Possible match - check actual file
            if let Some(res) = func(
                self.get_or_create_jar_provider(segment, &bloom.file_range)?
            )? {
                return Ok(Some(res))
            }
        }

        Ok(None)
    }
}
```

**Proposed Solution Option 2**: Hash index per static file

```rust
struct StaticFileHashIndex {
    // Hash -> offset in data file
    index: HashMap<TxHash, u64>,
    file_range: SegmentRangeInclusive,
}

impl NippyJarWriter {
    fn write_hash_index(&mut self, hashes: Vec<(TxHash, u64)>) -> Result<()> {
        // Write hash -> offset index to separate .idx file
        let index_path = self.jar.data_path().with_extension("idx");
        // Serialize HashMap to disk
    }
}
```

**Trade-offs**:
- Bloom filters: Small overhead (1-2MB per file), false positives possible
- Hash index: Larger overhead (32B per tx = ~160MB per file), no false positives

**Expected Improvement**:
- With bloom filters: 10-50x speedup for hash lookups (eliminate most file scans)
- With hash index: 100-1000x speedup (O(1) lookup within file)

**Implementation Effort**:
- Bloom filters: Medium (2-3 weeks)
- Hash index: Large (4-5 weeks, requires file format change)

**Priority**: 🟡 MEDIUM (HIGH for RPC nodes with frequent hash lookups)

---

## Sub-Issue 9: Parallel Transaction Hash Computation Uses Unnecessary Channels

**Location**: `crates/storage/provider/src/providers/static_file/manager.rs:1552-1610`

**Current Implementation**:
```rust
fn transaction_hashes_by_range(
    &self,
    tx_range: Range<TxNumber>,
) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
    let chunk_size = 100;
    let chunks = tx_range.clone().step_by(chunk_size).map(|start| ...);

    let mut channels = Vec::with_capacity(tx_range_size.div_ceil(chunk_size));

    for chunk_range in chunks {
        let (channel_tx, channel_rx) = mpsc::channel();  // ← Allocates per chunk
        channels.push(channel_rx);

        rayon::spawn(move || {
            let mut rlp_buf = Vec::with_capacity(128);  // ← Allocates per task
            // ... hash computation ...
            let _ = channel_tx.send(hash);
        });
    }

    // Sequential collection - blocks until all tasks complete
    for channel in channels {
        while let Ok(tx) = channel.recv() {
            tx_list.push(tx);
        }
    }

    Ok(tx_list)
}
```

**Problems**:
1. Creates mpsc channel for every 100-tx chunk
2. Allocates RLP buffer per rayon task
3. Collection phase is **sequential** (negates parallelism benefits)
4. No buffer pooling
5. Results collected unsorted, require additional sort

**Performance Impact**:
- For 100K transactions: 1000 channels + 1000 RLP buffers
- Channel allocation: ~200 bytes × 1000 = 200KB
- RLP buffers: 128 bytes × 1000 = 128KB
- Total: ~328KB of short-lived allocations
- Sequential collection adds latency

**Proposed Solution**:
Use rayon parallel iterators directly:

```rust
use rayon::prelude::*;
use once_cell::sync::Lazy;
use std::cell::RefCell;

// Thread-local buffer pool
thread_local! {
    static RLP_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(256));
}

fn transaction_hashes_by_range(
    &self,
    tx_range: Range<TxNumber>,
) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
    // Parallel iteration without channels
    let mut tx_list: Vec<_> = tx_range
        .into_par_iter()
        .filter_map(|tx_num| {
            // Reuse thread-local buffer
            RLP_BUFFER.with(|buf| {
                let mut buf = buf.borrow_mut();
                buf.clear();

                // Fetch transaction
                let tx = self.transaction_by_id(tx_num).ok()??;

                // Calculate hash
                tx.encode_2718(&mut buf);
                let hash = keccak256(&buf);

                Some((hash, tx_num))
            })
        })
        .collect();

    // Sort by tx_num if needed
    tx_list.par_sort_unstable_by_key(|(_, tx_num)| *tx_num);

    Ok(tx_list)
}
```

**Expected Improvement**:
- Eliminates 328KB allocations per 100K transactions
- Parallel collection instead of sequential
- 1.3-1.7x speedup for transaction hash queries

**Implementation Effort**: Small (1 week)

**Priority**: 🟡 MEDIUM

---

## Sub-Issue 10: Buffer Allocations in Hot Path

**Location**:
- `crates/storage/provider/src/providers/static_file/writer.rs:119-120, 143`
- `crates/storage/nippy-jar/src/writer.rs:77-79`

**Current Implementation**:

```rust
// In StaticFileProviderRW
pub struct StaticFileProviderRW<N> {
    writer: NippyJarWriter<SegmentHeader>,
    buf: Vec<u8>,  // Capacity: 100 bytes
    // ...
}

// In NippyJarWriter
pub struct NippyJarWriter<H> {
    tmp_buf: Vec<u8>,     // Capacity: 1,000,000 bytes (1MB)
    offsets: Vec<u64>,    // Capacity: 1,000,000 elements (8MB)
    // ...
}
```

**Problems**:
1. `StaticFileProviderRW` buffer starts at only 100 bytes
   - Typical transaction: 200-400 bytes
   - Typical receipt: 100-300 bytes
   - Buffer resizes frequently during writes
2. `NippyJarWriter` allocates 9MB upfront
   - May be wasteful for small write batches
   - No gradual growth strategy
3. No buffer pooling between writers
4. Compression temporary buffer grows unbounded

**Performance Impact**:
- Buffer resize on ~60-80% of transactions (if <100 bytes capacity)
- Each resize: allocation + copy
- For 5000 transactions: ~3000-4000 resizes
- Memory fragmentation from repeated allocations

**Proposed Solution**:

```rust
// Better buffer sizing based on profiling
pub struct StaticFileProviderRW<N> {
    buf: Vec<u8>,  // Capacity: 512 bytes (covers 90% of transactions)
}

// Gradual growth for NippyJarWriter
pub struct NippyJarWriter<H> {
    tmp_buf: Vec<u8>,     // Start: 4KB, grow as needed
    offsets: Vec<u64>,    // Start: 1000 elements, grow as needed
    // ...
}

impl NippyJarWriter {
    fn ensure_tmp_buf_capacity(&mut self, needed: usize) {
        if self.tmp_buf.capacity() < needed {
            let new_capacity = needed.next_power_of_two().max(4096);
            self.tmp_buf.reserve(new_capacity - self.tmp_buf.capacity());
        }
    }

    fn write_column(&mut self, value: &[u8]) -> Result<usize, NippyJarError> {
        self.uncompressed_row_size += value.len();

        if let Some(compression) = &self.jar.compressor {
            // Estimate compressed size (conservative)
            let estimated_size = compression.max_compressed_len(value.len());
            self.ensure_tmp_buf_capacity(self.tmp_buf.len() + estimated_size);

            let before = self.tmp_buf.len();
            let len = compression.compress_to(value, &mut self.tmp_buf)?;
            self.data_file.write_all(&self.tmp_buf[before..before + len])?;
            len
        } else {
            self.data_file.write_all(value)?;
            value.len()
        }
    }
}

// Optional: Buffer pool for reuse
struct BufferPool {
    buffers: Mutex<Vec<Vec<u8>>>,
    min_size: usize,
    max_pooled: usize,
}

impl BufferPool {
    fn acquire(&self, min_capacity: usize) -> Vec<u8> {
        let mut buffers = self.buffers.lock().unwrap();

        // Find suitable buffer from pool
        if let Some(pos) = buffers.iter().position(|buf| buf.capacity() >= min_capacity) {
            let mut buf = buffers.swap_remove(pos);
            buf.clear();
            return buf;
        }

        // Allocate new if pool empty
        Vec::with_capacity(min_capacity)
    }

    fn release(&self, mut buf: Vec<u8>) {
        buf.clear();

        let mut buffers = self.buffers.lock().unwrap();
        if buffers.len() < self.max_pooled && buf.capacity() >= self.min_size {
            buffers.push(buf);
        }
    }
}
```

**Expected Improvement**:
- Reduce allocations by 60-80%
- 1.1-1.2x write throughput improvement
- Reduced memory fragmentation

**Implementation Effort**: Small-Medium (1-2 weeks)

**Priority**: 🟡 MEDIUM

---

## Sub-Issue 11: Eager Index Initialization on Startup

**Location**: `crates/storage/provider/src/providers/static_file/manager.rs:666-713`

**Current Implementation**:
```rust
pub fn initialize_index(&self) -> ProviderResult<()> {
    let mut min_block = self.static_files_min_block.write();
    let mut max_block = self.static_files_max_block.write();
    let mut tx_index = self.static_files_tx_index.write();

    min_block.clear();
    max_block.clear();
    tx_index.clear();

    // Scans ALL static files on disk
    for (segment, ranges) in iter_static_files(&self.path).map_err(ProviderError::other)? {
        // Iterates all files for each segment
        if let Some((first_block_range, _)) = ranges.first() {
            min_block.insert(segment, *first_block_range);
        }
        if let Some((last_block_range, _)) = ranges.last() {
            max_block.insert(segment, last_block_range.end());
        }

        // Builds complete tx -> block_range index
        for (block_range, tx_range) in ranges {
            if let Some(tx_range) = tx_range {
                // BTreeMap insertions for every file
            }
        }
    }

    self.map.clear();  // Clear cached providers

    Ok(())
}
```

**Called**:
- On every `StaticFileProvider` creation
- On directory watch events (when files modified)

**Problems**:
1. Scans entire static files directory on startup
2. Reads all config files (NippyJar metadata)
3. Builds full tx→block index upfront
4. For 10M blocks: 20+ file metadata reads
5. Adds startup latency (50-200ms depending on file count)

**Performance Impact**:
- Node startup: +50-200ms
- Directory watch trigger: +50-200ms per event
- Memory: Full index always in memory (even for cold data)

**Proposed Solution**:
Lazy index building with incremental construction:

```rust
struct LazyStaticFileIndex {
    // Core indices (always loaded)
    max_block: RwLock<HashMap<StaticFileSegment, BlockNumber>>,

    // Lazy indices (loaded on-demand)
    tx_index: RwLock<Option<SegmentRanges>>,
    tx_index_state: AtomicU8,  // 0=uninitialized, 1=loading, 2=loaded
}

impl LazyStaticFileIndex {
    fn ensure_tx_index_loaded(&self) -> ProviderResult<()> {
        // Fast path: already loaded
        if self.tx_index_state.load(Ordering::Acquire) == 2 {
            return Ok(());
        }

        // Slow path: need to load
        let mut guard = self.tx_index.write();
        if guard.is_some() {
            return Ok(());  // Another thread loaded it
        }

        // Load index
        *guard = Some(self.build_tx_index()?);
        self.tx_index_state.store(2, Ordering::Release);

        Ok(())
    }

    fn get_tx_range(&self, segment: StaticFileSegment, tx: u64) -> ProviderResult<Option<SegmentRangeInclusive>> {
        self.ensure_tx_index_loaded()?;

        let index = self.tx_index.read();
        Ok(index.as_ref()?.get(&segment).and_then(|ranges| {
            // Search for tx in ranges
        }))
    }
}

impl StaticFileProviderInner {
    fn initialize_index_lazy(&self) -> ProviderResult<()> {
        // Only scan for max blocks (minimal work)
        let static_files = iter_static_files(&self.path).map_err(ProviderError::other)?;

        let mut max_block = self.static_files_max_block.write();
        max_block.clear();

        for (segment, ranges) in static_files {
            if let Some((last_block_range, _)) = ranges.last() {
                max_block.insert(segment, last_block_range.end());
            }
        }

        // tx_index will be built on first access

        Ok(())
    }
}
```

**Expected Improvement**:
- Startup time: 50-200ms reduction
- Memory: Only load indices when needed
- For read-only nodes that don't use tx lookups: significant memory savings

**Implementation Effort**: Medium (2 weeks)

**Priority**: 🟢 LOW (unless startup time is critical)

---

## Sub-Issue 12: Headers Segment Cursor Recreation

**Location**: `crates/static-file/static-file/src/segments/headers.rs:33-48`

**Current Implementation**:
```rust
fn copy_to_static_files(
    &self,
    provider: Provider,
    block_range: RangeInclusive<BlockNumber>,
) -> ProviderResult<()> {
    let static_file_provider = provider.static_file_provider();
    let mut static_file_writer = static_file_provider
        .get_writer(*block_range.start(), StaticFileSegment::Headers)?;

    // Create 3 separate cursors
    let mut headers_cursor = provider
        .tx_ref()
        .cursor_read::<tables::Headers>()?;
    let headers_walker = headers_cursor.walk_range(block_range.clone())?;

    let mut header_td_cursor = provider
        .tx_ref()
        .cursor_read::<tables::HeaderTerminalDifficulties>()?;
    let header_td_walker = header_td_cursor.walk_range(block_range.clone())?;

    let mut canonical_headers_cursor = provider
        .tx_ref()
        .cursor_read::<tables::CanonicalHeaders>()?;
    let canonical_headers_walker = canonical_headers_cursor.walk_range(block_range)?;

    // Zip and iterate
    for ((header_entry, header_td_entry), canonical_header_entry) in
        headers_walker.zip(header_td_walker).zip(canonical_headers_walker)
    {
        let (header_block, header) = header_entry?;
        let (header_td_block, header_td) = header_td_entry?;
        let (canonical_header_block, canonical_header) = canonical_header_entry?;

        static_file_writer.append_header(&header, header_td.0, &canonical_header)?;
    }

    Ok(())
}
```

**Problems**:
1. Creates 3 separate cursors (one per table)
2. Cursors are **not reused** across multiple segment runs
3. Each cursor creation has overhead (MDBX transaction setup)
4. Good: Uses `zip()` for parallel iteration
5. Bad: No cursor pooling between segment runs

**Performance Impact**:
- Cursor creation: ~10-50μs per cursor × 3 = 30-150μs
- Called once per segment run, but could be reused
- Minor impact, but adds up with frequent runs

**Proposed Solution**:
Cursor pooling for segment workers:

```rust
struct SegmentCursorPool<Provider> {
    headers_cursor: Option<DbCursorRO<tables::Headers>>,
    header_td_cursor: Option<DbCursorRO<tables::HeaderTerminalDifficulties>>,
    canonical_headers_cursor: Option<DbCursorRO<tables::CanonicalHeaders>>,
    provider: Provider,
}

impl<Provider> SegmentCursorPool<Provider> {
    fn get_or_create_headers_cursor(&mut self) -> ProviderResult<&mut DbCursorRO<tables::Headers>> {
        if self.headers_cursor.is_none() {
            self.headers_cursor = Some(
                self.provider.tx_ref().cursor_read::<tables::Headers>()?
            );
        }
        Ok(self.headers_cursor.as_mut().unwrap())
    }

    // Similar for other cursors...

    fn reset(&mut self) {
        // Clear cursors for new transaction
        self.headers_cursor = None;
        self.header_td_cursor = None;
        self.canonical_headers_cursor = None;
    }
}

impl Segment<Provider> for Headers {
    fn copy_to_static_files_with_pool(
        &self,
        cursor_pool: &mut SegmentCursorPool<Provider>,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let headers_cursor = cursor_pool.get_or_create_headers_cursor()?;
        let header_td_cursor = cursor_pool.get_or_create_header_td_cursor()?;
        let canonical_headers_cursor = cursor_pool.get_or_create_canonical_headers_cursor()?;

        // ... rest of logic ...
    }
}
```

**Expected Improvement**:
- Marginal (1.01-1.05x) for headers segment
- More significant if segment runs are frequent

**Implementation Effort**: Small (3-5 days)

**Priority**: 🟢 LOW

---

## Sub-Issue 13: No Prewarming of Static File Providers

**Location**: Implicit (not currently implemented)

**Current Behavior**:
- Static file providers loaded lazily on first access
- First query to recent blocks: cold start penalty
- DashMap cache miss = disk I/O to load NippyJar

**Problems**:
1. First RPC query after node start: 10-50ms additional latency
2. First query after file rotation: similar penalty
3. No anticipation of which files will be accessed
4. Cold start impacts user experience

**Performance Impact**:
- First query to recent blocks: +10-50ms
- First query after file rotation: +10-50ms
- Affects time-sensitive operations (block building, RPC)

**Proposed Solution**:
Background prewarming thread:

```rust
struct StaticFilePrewarmer {
    provider: Arc<StaticFileProviderInner>,
    prewarm_window: BlockNumber,  // e.g., 1M blocks
    prewarm_interval: Duration,
}

impl StaticFilePrewarmer {
    fn start(self) -> JoinHandle<()> {
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(self.prewarm_interval);

                if let Err(err) = self.prewarm_recent_files() {
                    warn!(target: "static_file", ?err, "Failed to prewarm static files");
                }
            }
        })
    }

    fn prewarm_recent_files(&self) -> ProviderResult<()> {
        for segment in [StaticFileSegment::Headers, StaticFileSegment::Transactions, StaticFileSegment::Receipts] {
            let Some(highest_block) = self.provider.get_highest_static_file_block(segment) else {
                continue;
            };

            let prewarm_start = highest_block.saturating_sub(self.prewarm_window);

            // Load all providers in recent window
            let mut current_range = self.provider.find_fixed_range(prewarm_start);
            while current_range.start() <= highest_block {
                // Trigger provider load (cached in DashMap)
                let _ = self.provider.get_or_create_jar_provider(segment, &current_range);

                current_range = SegmentRangeInclusive::new(
                    current_range.start() + self.provider.blocks_per_file,
                    current_range.end() + self.provider.blocks_per_file,
                );
            }
        }

        Ok(())
    }
}

impl StaticFileProvider {
    pub fn with_prewarming(self, prewarm_window: BlockNumber) -> Self {
        let prewarmer = StaticFilePrewarmer {
            provider: self.0.clone(),
            prewarm_window,
            prewarm_interval: Duration::from_secs(60),
        };

        prewarmer.start();

        self
    }
}
```

**Configuration**:
- Default prewarm window: 1M blocks (2 static files)
- Prewarm interval: 60 seconds
- Run in background, low priority

**Expected Improvement**:
- Eliminates 10-50ms cold start penalty
- Smooth first-query experience
- Negligible overhead (background thread)

**Implementation Effort**: Small (1 week)

**Priority**: 🟢 LOW (MEDIUM for production RPC nodes)

---

## Implementation Roadmap

### Phase 1: Quick Wins (2-3 weeks, Target: 20-30% improvement)
**Goal**: Reduce 5s write time to ~3.5-4s

1. ✅ **Sub-Issue 10**: Buffer size tuning (3 days)
2. ✅ **Sub-Issue 5**: Multi-block receipt batching (1 week)
3. ✅ **Sub-Issue 9**: Channel-free parallel hash computation (1 week)

**Expected Result**: 5s → 3.5-4s for 5000-tx block

---

### Phase 2: Parallelization (3-4 weeks, Target: 100-150% improvement)
**Goal**: Reduce write time to ~1.5-2s

1. ✅ **Sub-Issue 1**: Parallel block preparation pipeline (2-3 weeks)
2. ✅ **Sub-Issue 3**: Pre-allocate next file before rotation (1 week)

**Expected Result**: 3.5s → 1.5-2s for 5000-tx block

---

### Phase 3: Async I/O (3-4 weeks, Target: 50% additional improvement)
**Goal**: Reduce write time to ~1s, achieve sub-second for Gravity

1. ✅ **Sub-Issue 2**: Async fsync operations (3-4 weeks)
2. ✅ **Sub-Issue 4**: Batched async index updates (2 weeks, can overlap)

**Expected Result**: 1.5s → 0.8-1s for 5000-tx block ✅ **SUB-SECOND GOAL**

---

### Phase 4: Read Optimizations (3-4 weeks)
**Goal**: Improve RPC query performance

1. ✅ **Sub-Issue 6**: Decompression caching (2 weeks)
2. ✅ **Sub-Issue 7**: Provider prefetching at boundaries (2 weeks)
3. ✅ **Sub-Issue 13**: Provider prewarming (1 week, can overlap)

**Expected Result**: 2-5x read speedup for hot data

---

### Phase 5: Advanced Optimizations (4-6 weeks)
**Goal**: Further improvements for specific workloads

1. ✅ **Sub-Issue 8**: Bloom filters for hash lookups (2-3 weeks)
2. ✅ **Sub-Issue 11**: Lazy index initialization (2 weeks, can overlap)
3. ✅ **Sub-Issue 12**: Cursor pooling (1 week)

**Expected Result**: 10-50x improvement for hash lookups, reduced memory usage

---

## Success Metrics

### Primary Goal: Gravity Sub-Second Block Persistence
- **Current**: 5 seconds for 5000-tx block
- **Target**: <1 second for 5000-tx block
- **Phases Required**: Phase 1 + Phase 2 + Phase 3

### Secondary Goals
- Eliminate OOM from memory backlog
- Reduce block accumulation in `CanonicalInMemoryState`
- Improve RPC query latency (especially for recent blocks)
- Reduce startup time and cold-start penalties

### Monitoring
- Track static file write latency via metrics
- Monitor `CanonicalInMemoryState` memory usage
- Measure fsync time overhead
- Track DB cursor creation rate
- Monitor DashMap cache hit rate

---

## Alternative Approaches Considered

### 1. Complete Async Rewrite
**Rejected**: Too invasive, append-only file writes must be sequential

### 2. Separate Write Thread Pool
**Considered**: Similar benefits to current proposal but more complex

### 3. Write Batching at StaticFileProducer Level
**Rejected**: Batching already happens at segment level, additional batching adds minimal benefit

### 4. Memory-Mapped Writes
**Rejected**: mmap doesn't provide better control over fsync, still needs explicit msync

### 5. Custom File Format Instead of NippyJar
**Rejected**: NippyJar is well-tested, custom format too risky

---

## Open Questions

1. **fsync Safety**: Can we safely defer fsync for finalized blocks?
   - Need to ensure consensus safety
   - Coordinate with engine API finalization signals

2. **Buffer Pool Implementation**: Global pool vs per-writer?
   - Trade-off: contention vs memory usage

3. **Bloom Filter False Positive Rate**: What's acceptable?
   - Affects bloom filter size vs query performance

4. **Prefetch Window Size**: How many blocks to prefetch?
   - Depends on workload: sequential scan vs random access

5. **Decompression Cache Size**: Memory budget?
   - Need to profile typical working set size

---

## Additional Context

### Related Issues
- Gravity sub-second block generation requirements
- Memory pressure in `CanonicalInMemoryState`
- OOM issues with 10M accounts + high throughput

### Dependencies
- No external dependencies required
- All changes contained within reth codebase
- Backward compatible (can be implemented incrementally)

### Testing Strategy
- Benchmark each optimization independently
- Integration tests for parallel pipelines
- Stress tests with 10M accounts + 5000 tx/block
- Verify consensus safety of async fsync
- Validate data integrity across all changes

---

## Labels
- `C-perf`: Performance improvement
- `C-tracking-issue`: Tracking issue
- `A-storage`: Storage layer
- `A-static-files`: Static files specific
- `P-high`: High priority (Gravity blocking)
