# Streaming Lthash State Root Design

This document describes the TIP-1078 Lthash performance experiment.

The goal is to measure the performance impact of replacing Merkle Patricia Trie
state root computation with an Lthash accumulator. This design intentionally does
not cover production migration, shadow-build, checkpoint trust, snap sync, or
historical MPT proof compatibility. It also intentionally does not wire the
computed Lthash checksum into header validation yet.

The implementation should be split into three JJ changes:

1. `Add Lthash foundation`
2. `Generalize state IO pool`
3. `Add streaming Lthash pipeline`

The first change may amend the existing Lthash accumulator change. The second
change should stand alone. The third change should include the Lthash task and
all builder/validator wiring.

## Current Constraints

- MPT preservation is not a goal for this experiment.
- Header `state_root` enforcement is paused. Compute the Lthash root and
  accumulator, attach the accumulator to `ExecutedBlock`, record metrics, but do
  not reject a block because the header does not match the Lthash checksum.
- The benchmark path must be streaming. Do not compute Lthash from final
  `BundleState` after execution as the main path. That would measure a
  post-execution serial cost, not the intended overlapped root pipeline.
- Do not start per-block ad hoc threads for old-state reads. Use a persistent
  pool with a per-block lifecycle, like the existing BAL prewarm pool.
- BAL and non-BAL must feed the same Lthash replacement model. BAL-specific
  sparse-trie flags must not disable Lthash.
- For this experiment, the first active block may use a zero accumulator as its
  parent accumulator. This is not the production TIP-1078 activation model.
- Persisting the full accumulator to a DB table is out of scope for the first
  benchmark pass. Use in-memory parent accumulators where available.

## TIP-1078 Encoding

Use the local TIP draft as the source of truth:

`/Users/tempo-pep/dev/tempo/tips/tip-1078.md`

The implementation must not invent a different element encoding.

### Lthash Construction

- The accumulator is 2048 bytes.
- The accumulator is interpreted as 1024 little-endian `u16` lanes.
- The identity accumulator is all zero bytes.
- `expand(element) = BLAKE3-XOF(element, 2048)`.
- `add` and `subtract` use wrapping lane-wise arithmetic modulo `2^16`.
- `checksum(acc) = BLAKE3-256(acc)`, where `acc` is serialized as 2048 raw
  little-endian lane bytes.

### Account Element

For each live account, add exactly one account element:

```text
0x00 || hashed_address || nonce || balance || code_hash
```

Fields:

- `0x00`: account domain byte.
- `hashed_address`: `keccak256(plain_address)`, 32 bytes.
- `nonce`: `u64` big-endian, 8 bytes.
- `balance`: `U256` big-endian, 32 bytes.
- `code_hash`: 32 bytes. Use `KECCAK_EMPTY` for an account with no code.

Total length: 105 bytes.

Missing, deleted, or non-live accounts are absent from the set. There is no
"zero account" element.

### Storage Element

For each nonzero storage slot, add exactly one storage element:

```text
0x01 || hashed_address || hashed_slot || value
```

Fields:

- `0x01`: storage domain byte.
- `hashed_address`: `keccak256(plain_address)`, 32 bytes.
- `hashed_slot`: `keccak256(plain_slot)`, 32 bytes.
- `value`: `U256` big-endian, 32 bytes.

Total length: 97 bytes.

Zero-valued storage slots are absent from the set. There is no "zero storage"
element.

## Change 1: Add Lthash Foundation

### Files

- `crates/chain-state/src/lthash.rs`
- `crates/chain-state/src/lib.rs`
- `crates/chain-state/src/in_memory.rs`
- `crates/chain-state/Cargo.toml`
- `Cargo.lock`

### Dependency

Add `blake3` to `reth-chain-state`. Do not add Commonware.

Run:

```bash
zepter
make lint-toml
```

`zepter` may require `blake3/serde` to be propagated under the
`reth-chain-state/serde` feature.

### Accumulator Type

Define `LthashAccumulator` in `reth-chain-state`.

Required behavior:

- Store the full accumulator state as 1024 `u16` lanes or an equivalent 2048
  byte representation.
- Convert raw bytes to lanes as little-endian.
- Convert lanes to raw bytes as little-endian.
- Provide an empty accumulator.
- Add an element by BLAKE3-XOF expanding it to 2048 bytes and adding all lanes.
- Subtract an element by BLAKE3-XOF expanding it to 2048 bytes and subtracting
  all lanes.
- Combine accumulators by lane-wise wrapping addition.
- Return `B256` checksum from `BLAKE3-256(raw_accumulator_bytes)`.

Suggested API:

```rust
pub const LTHASH_ACCUMULATOR_LEN: usize = 2048;

pub struct LthashAccumulator(...);

impl LthashAccumulator {
    pub const fn zero() -> Self;
    pub const fn from_bytes(bytes: [u8; LTHASH_ACCUMULATOR_LEN]) -> Self;
    pub const fn to_bytes(&self) -> [u8; LTHASH_ACCUMULATOR_LEN];
    pub fn add(&mut self, element: impl AsRef<[u8]>);
    pub fn subtract(&mut self, element: impl AsRef<[u8]>);
    pub fn combine(&mut self, other: &Self);
    pub fn checksum(&self) -> B256;
    pub fn is_zero(&self) -> bool;
}
```

### Element Encoding Helpers

Add helpers in the same module. They should be reusable by the streaming task
and by tests.

Suggested shape:

```rust
pub const LTHASH_ACCOUNT_ELEMENT_LEN: usize = 105;
pub const LTHASH_STORAGE_ELEMENT_LEN: usize = 97;

pub fn account_element(
    hashed_address: B256,
    account: Option<&Account>,
) -> Option<[u8; LTHASH_ACCOUNT_ELEMENT_LEN]>;

pub fn storage_element(
    hashed_address: B256,
    hashed_slot: B256,
    value: U256,
) -> Option<[u8; LTHASH_STORAGE_ELEMENT_LEN]>;
```

Rules:

- `account_element(None)` returns `None`.
- If an account is present but has no bytecode hash, encode `KECCAK_EMPTY`.
- If the local account type can represent an empty/deleted account, return
  `None` for that state.
- `storage_element(..., U256::ZERO)` returns `None`.
- Nonzero storage values return a 97-byte element.

Use `alloy_consensus::constants::KECCAK_EMPTY` for the empty code hash.

### Test Vectors

Add tests from the TIP draft:

- empty set checksum
- single account
- account plus storage
- account replacement

Also add local behavior tests:

- raw accumulator bytes round-trip
- add/subtract cancellation
- account domain does not collide with storage domain
- zero storage is absent
- missing/deleted account is absent

### ExecutedBlock Carrier

Add an optional accumulator to `ExecutedBlock`:

```rust
pub lthash_accumulator: Option<Arc<LthashAccumulator>>,
```

Constructors and `Default` should set it to `None`.

Add:

```rust
pub fn lthash_accumulator(&self) -> Option<&LthashAccumulator>;
pub fn lthash_accumulator_handle(&self) -> Option<Arc<LthashAccumulator>>;
pub fn with_lthash_accumulator(
    self,
    accumulator: impl Into<Arc<LthashAccumulator>>,
) -> Self;
```

`PartialEq` must continue to ignore auxiliary derived state. The accumulator
does not define block identity.

## Change 2: Generalize State IO Pool

### Existing Starting Point

The existing file:

`crates/engine/tree/src/tree/payload_processor/bal_prewarm_pool.rs`

already has the right lifecycle pattern:

- long-lived worker threads owned by `PayloadProcessor`
- one FIFO queue per worker
- `BeginBlock` opens a parent-state provider per worker
- providers are wrapped in `CachedStateProvider`
- warm requests use that provider/cache
- `EndBlock` drops the provider/read transaction

That lifecycle should be reused. Do not move Lthash old-state reads into the
Lthash task itself.

### Rename Or Generalize

Rename conceptually to `StateIoPool`. The file can either be renamed or kept
with a broader type inside it, but the public type should not remain BAL-only if
it is used by non-BAL Lthash.

Suggested file:

`crates/engine/tree/src/tree/payload_processor/state_io_pool.rs`

Suggested type:

```rust
pub(crate) struct StateIoPool { ... }
```

Keep the pool owned by `PayloadProcessor`. It must be created once and reused
across blocks. Do not spawn old-read workers per block.

### Provider Builder

Keep the current type-erased provider builder:

```rust
type BuildProviderFn = dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync;
```

Every worker should build its own provider at `BeginBlock`.

Wrap with:

```rust
CachedStateProvider::new_prewarm(inner, caches)
```

This preserves cache sharing with prewarm and avoids duplicating provider/cache
initialization in the Lthash task.

### Message Types

The pool must support both warming and value-returning old reads.

Internal worker messages:

```rust
enum StateIoMsg {
    BeginBlock {
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        old_value_tx: crossbeam_channel::Sender<StateIoReadResult>,
    },
    Warm(StateIoWarmTarget),
    Read(StateIoReadTarget),
    EndBlock {
        ack: crossbeam_channel::Sender<()>,
    },
}
```

Targets:

```rust
enum StateIoWarmTarget {
    Account(Address),
    Storage(Address, StorageKey),
}

enum StateIoReadTarget {
    Account {
        address: Address,
        hashed_address: B256,
    },
    Storage {
        address: Address,
        hashed_address: B256,
        slot: StorageKey,
        hashed_slot: B256,
    },
}
```

Results:

```rust
enum StateIoReadResult {
    Account {
        hashed_address: B256,
        account: ProviderResult<Option<Account>>,
    },
    Storage {
        hashed_address: B256,
        hashed_slot: B256,
        value: ProviderResult<U256>,
    },
}
```

Use `U256::ZERO` for absent storage if the provider returns `None`.

The result carries hashed keys because the Lthash task's replacement table is
keyed by hashed address and hashed slot. The worker still needs plain keys for
provider reads.

### Public API

Suggested API:

```rust
impl StateIoPool {
    pub(crate) fn new(num_threads: usize) -> Arc<Self>;

    pub(crate) fn begin_block(
        &self,
        build: Arc<BuildProviderFn>,
        caches: ExecutionCache,
        old_value_tx: crossbeam_channel::Sender<StateIoReadResult>,
    );

    pub(crate) fn warm_account(&self, address: Address);
    pub(crate) fn warm_storage(&self, address: Address, slot: StorageKey);

    pub(crate) fn read_account(&self, address: Address, hashed_address: B256);

    pub(crate) fn read_storage(
        &self,
        address: Address,
        hashed_address: B256,
        slot: StorageKey,
        hashed_slot: B256,
    );

    pub(crate) fn end_block(&self) -> StateIoBarrier;
}
```

`StateIoBarrier` should wait for all worker acks:

```rust
pub(crate) struct StateIoBarrier {
    rx: crossbeam_channel::Receiver<()>,
    expected: usize,
}

impl StateIoBarrier {
    pub(crate) fn wait(self);
}
```

`EndBlock` must be FIFO per worker. A worker sends its ack only after it drains
all reads queued before the `EndBlock` message and drops its provider. The
Lthash task must wait on this barrier before final checksum.

### Warming Behavior

Preserve current warming behavior:

- `WarmAccount` reads `basic_account`.
- If the account has a non-empty code hash, read bytecode by hash.
- `WarmStorage` reads storage.

This is still needed for BAL prewarm.

### Read Behavior

For `ReadAccount`:

- call `basic_account(&address)`
- send the result to `old_value_tx`
- do not read bytecode; Lthash account element only needs code hash

For `ReadStorage`:

- call `storage(address, slot)`
- convert `None` to `U256::ZERO`
- send the result to `old_value_tx`

### Error Behavior

For the benchmark, a provider error should be sent back to the Lthash task and
make Lthash finalization fail for the block. Do not silently treat read errors
as zero.

### Tests

Add focused tests where possible:

- `EndBlock` waits for queued reads before ack.
- account read returns an old account result.
- storage read returns zero for absent storage.
- warm-only requests still work without a result consumer.
- provider build failure produces no panic and sends/read errors consistently.

## Change 3: Add Streaming Lthash Pipeline

### Files

Likely files:

- `crates/engine/tree/src/tree/payload_processor/lthash.rs`
- `crates/engine/tree/src/tree/payload_processor/mod.rs`
- `crates/engine/tree/src/tree/payload_processor/prewarm.rs`
- `crates/engine/tree/src/tree/payload_validator.rs`
- payload builder integration files that currently await sparse-trie root
- `crates/engine/tree/src/tree/metrics.rs`

### Core Model

For each changed account or storage key, the final block update is:

```text
parent_accumulator
  - original live element, if any
  + final live element, if any
```

It is not enough to add every value observed in the stream. A key can change
multiple times in one block. Only the parent value and final block value matter.

The task may optimistically add a current new value while the stream is still
open. If a later value for the same key arrives, it must subtract the previously
added new element and mark the latest value dirty again.

### Lthash Messages

Do not reuse `StateRootMessage` or `HashedPostState` as the Lthash input.
Sparse trie and Lthash need different data.

Suggested input messages:

```rust
enum LthashMessage {
    AccountTouched {
        address: Address,
        hashed_address: B256,
        new_account: Option<Account>,
    },
    StorageTouched {
        address: Address,
        hashed_address: B256,
        slot: StorageKey,
        hashed_slot: B256,
        new_value: U256,
    },
    FinishedUpdates,
}
```

`new_account: None` means the account is absent/deleted after the latest
observed update.

`new_value == U256::ZERO` means the storage element is absent after the latest
observed update.

### Old Read Results

The Lthash task also receives `StateIoReadResult` from `StateIoPool`.

The task should not construct providers and should not read MDBX directly.

### Key State

Keep separate maps:

```rust
accounts: HashMap<B256, AccountEntry>
storages: HashMap<(B256, B256), StorageEntry>
```

Suggested account entry:

```rust
struct AccountEntry {
    address: Address,
    old_requested: bool,
    old_done: bool,
    latest_new: Option<Account>,
    added_new: Option<[u8; LTHASH_ACCOUNT_ELEMENT_LEN]>,
    dirty_new: bool,
}
```

Suggested storage entry:

```rust
struct StorageEntry {
    address: Address,
    slot: StorageKey,
    old_requested: bool,
    old_done: bool,
    latest_new: U256,
    added_new: Option<[u8; LTHASH_STORAGE_ELEMENT_LEN]>,
    dirty_new: bool,
}
```

If memory pressure becomes visible, store expanded 2048-byte lattice points
instead of element bytes only when needed, or recompute on supersession. For the
first benchmark, storing element bytes is simpler and enough. The expensive part
is expansion, so keeping enough data to subtract an optimistic addition matters.

### First Touch

On first account touch:

1. Insert an account entry.
2. Queue `StateIoPool::read_account(address, hashed_address)`.
3. Store `latest_new`.
4. Mark `dirty_new`.

On first storage touch:

1. Insert a storage entry.
2. Queue `StateIoPool::read_storage(address, hashed_address, slot, hashed_slot)`.
3. Store `latest_new`.
4. Mark `dirty_new`.

On later touches:

1. If `added_new` exists, subtract that element from the accumulator and clear it.
2. Replace `latest_new`.
3. Mark `dirty_new`.

Do not request the old value again.

### Old Result Handling

For old account result:

1. Find the account entry by `hashed_address`.
2. If result is an error, record task failure.
3. Encode the old account element.
4. If present, subtract it immediately.
5. Mark `old_done`.

For old storage result:

1. Find the storage entry by `(hashed_address, hashed_slot)`.
2. If result is an error, record task failure.
3. Encode the old storage element.
4. If present, subtract it immediately.
5. Mark `old_done`.

It is valid for old subtraction to happen before or after new optimistic
addition. Lthash addition/subtraction is commutative.

### Optimistic New Additions

The task loop should process dirty additions when it is idle.

Suggested loop behavior:

```text
while not finished:
    drain available Lthash messages
    drain available old read results
    if no messages were available:
        process some dirty new additions
```

At finish:

```text
wait for producer FinishedUpdates
call StateIoPool::end_block()
wait for StateIoBarrier
drain all old read results
process all dirty additions
return outcome
```

If an optimistic addition is superseded by a later message, subtract the old
optimistic element before marking the key dirty again.

### Parallel Hashing

Each element expansion writes 2048 bytes. Parallelizing every item separately
can be slower than serial execution.

Use thresholds:

- serial for small dirty queues
- parallel for larger batches

Suggested behavior:

```text
const LTHASH_PARALLEL_EXPAND_THRESHOLD: usize = 64;
const LTHASH_DIRTY_BATCH_SIZE: usize = 256;
```

The exact values can be tuned. Start conservative.

Parallel expansion should produce a list of element bytes or expanded lattice
points, then apply the resulting accumulator updates on the owning task thread.
Do not mutate the accumulator concurrently.

Use existing Reth task infrastructure or Rayon pools already available in the
payload processor. Do not spawn per-block raw threads.

### Outcome

Define:

```rust
pub(crate) struct LthashOutcome {
    pub root: B256,
    pub accumulator: Arc<LthashAccumulator>,
    pub account_updates: usize,
    pub storage_updates: usize,
}
```

The root is `accumulator.checksum()`.

### Handle

The handle should mirror the useful parts of `StateRootHandle`, but stay
Lthash-specific:

```rust
pub(crate) struct LthashHandle {
    updates_tx: crossbeam_channel::Sender<LthashMessage>,
    outcome_rx: std::sync::mpsc::Receiver<Result<LthashOutcome, LthashError>>,
}

impl LthashHandle {
    pub(crate) fn state_hook(&self) -> impl OnStateHook;
    pub(crate) fn updates_tx(&self) -> crossbeam_channel::Sender<LthashMessage>;
    pub(crate) fn outcome(self) -> Result<LthashOutcome, LthashError>;
}
```

The handle can be part of `PayloadHandle`.

### Parent Accumulator

For the benchmark:

- If parent block is in memory and has an accumulator on `ExecutedBlock`, use it.
- If parent accumulator is unavailable, use `LthashAccumulator::zero()`.

This is a deliberate experiment shortcut. The production TIP requires a
prepared parent accumulator at activation.

Do not read a persisted accumulator table in this pass.

### Non-BAL Producer

Use the execution state hook.

`StateRootHandle::state_hook()` already sends `EvmState` into sparse trie. Add
an Lthash hook beside it.

The hook must convert each `EvmState` into Lthash touches:

- for every changed account, send `AccountTouched`
- for every changed storage slot, send `StorageTouched`

The hook must use the latest account/storage value visible after that
transaction.

Important invariant to verify with tests:

- The value sent as `new_*` must be the latest value after the transaction.
- The task coalesces multiple transactions for the same key, so it must not rely
  on each message being final.

Hash plain keys in the producer:

- `hashed_address = keccak256(address)`
- `hashed_slot = keccak256(slot.to_be_bytes::<32>())`

### BAL Producer

BAL prewarm currently streams `HashedPostState` for sparse trie from
`send_bal_hashed_state`.

Add a sibling Lthash BAL stream:

- for each `AccountChanges`, compute the account's final post value from the BAL
  fields and parent account fallback
- send `AccountTouched`
- for each changed storage slot, take the last change's `new_value`
- send `StorageTouched`

The BAL producer should use the same parent-state provider/cache layer for
fallback fields as sparse-trie BAL streaming already does.

Do not gate this with `disable_bal_parallel_state_root`. That flag is about
BAL-driven sparse-trie state root computation. Lthash needs its own config gate
if a gate is required.

### Builder Integration

The builder must use the same streaming pipeline. Do not wait for final
`BundleState` and compute Lthash afterward.

Expected shape:

1. Spawn Lthash before transaction execution starts.
2. Install the Lthash state hook into execution.
3. Stream updates while transactions execute.
4. Await `LthashOutcome` with the other post-execution work.
5. Attach accumulator to the built/executed block once the builder path creates
   an `ExecutedBlock`.

For the paused-header phase, the builder may log or metric the computed Lthash
root without putting it in the header.

### Validator Integration

In `payload_validator.rs`, after execution and before returning
`ValidationOutput`:

1. Await Lthash outcome.
2. Record metrics.
3. Attach accumulator:

```rust
let executed_block = executed_block.with_lthash_accumulator(outcome.accumulator);
```

Do not compare `outcome.root` to `block.header().state_root()` in this phase.

The existing sparse-trie/MPT validation remains in place until a later change
explicitly switches the header root.

### PayloadHandle Integration

`PayloadHandle` should carry both root handles:

```rust
state_root_handle: Option<StateRootHandle>,
lthash_handle: Option<LthashHandle>,
```

State hooks should be composed carefully. If both sparse trie and Lthash are
enabled, execution should call both hooks for the same `EvmState`.

Avoid cloning expensive state more than needed. If `EvmState` must be consumed
by one hook, build a combined hook at the call site that forwards references or
clones only when necessary.

### Account Deletion And Storage Wipe

This is the main correctness edge case.

TIP-1078 says account deletion removes all nonzero storage elements for that
account before post-deletion storage effects required by the active EVM rules
are applied.

A streaming task that only sees touched storage slots cannot subtract untouched
nonzero storage slots for a deleted account.

For the benchmark, implement an explicit slow path:

- detect account deletion/selfdestruct in the stream
- enqueue a storage wipe request for the account
- the state IO layer enumerates parent nonzero storage for that account, or
  falls back to a synchronous provider path if enumeration is only available
  there
- send old storage results to Lthash so each nonzero old slot can be subtracted

If enumeration is too large to implement in the first pass, the implementation
must explicitly reject or fallback for blocks that delete accounts with storage.
Do not silently produce a wrong accumulator.

### Error Handling

Define `LthashError`.

It should cover:

- old account read failure
- old storage read failure
- storage wipe/enumeration unsupported or failed
- Lthash task channel closed
- result channel closed

For this phase, a Lthash error should fail the experimental path loudly. Do not
fallback to final `BundleState` silently, because that hides the performance
behavior the experiment is meant to measure.

### Metrics

Add metrics under the engine tree metrics area. Minimum useful metrics:

- Lthash task total duration
- old account reads requested
- old storage reads requested
- old read wait duration
- account touches received
- storage touches received
- dirty account additions processed
- dirty storage additions processed
- optimistic additions superseded
- finalization wait duration
- Lthash checksum/root value in trace logs

Metrics should make it possible to answer:

- how much work was overlapped with execution
- how much time was spent waiting for old reads at finalization
- whether superseded optimistic additions are common
- whether hashing is CPU-bound enough to parallelize

### Tests

Add tests at three levels.

Foundation tests:

- TIP vectors.
- raw accumulator byte round-trip.
- add/subtract cancellation.
- zero storage absence.
- missing/deleted account absence.

Task unit tests with fake state IO results:

- first touch requests one old read.
- repeated touches for the same key request one old read.
- old subtraction can arrive before new addition.
- old subtraction can arrive after new addition.
- superseded optimistic account addition is subtracted.
- superseded optimistic storage addition is subtracted.
- final root equals a fresh accumulator built from final elements.
- old read error returns `LthashError`.

Integration-style tests:

- non-BAL streaming over multiple transactions equals a fresh build from final
  state.
- BAL-derived stream equals non-BAL stream for the same block, if a BAL fixture
  exists.
- parent accumulator continuation: applying block B to accumulator A equals
  fresh build for post-B state.
- missing parent accumulator uses zero only under the experiment shortcut.

### Implementation Order

Use this order inside the third change:

1. Add `lthash.rs` task module with fake/testable IO interfaces.
2. Add `StateIoPool` read-result integration.
3. Add non-BAL hook and tests.
4. Add BAL producer and tests.
5. Add builder/validator awaiting and `ExecutedBlock` attachment.
6. Add metrics.
7. Run targeted checks.

### Targeted Checks

Run these before handing off:

```bash
cargo test -p reth-chain-state lthash
cargo check -p reth-chain-state
cargo check -p reth-chain-state --features serde
cargo check -p reth-provider -p reth-engine-tree --tests
zepter
make lint-toml
cargo +nightly fmt --all --check
```

If builder files are touched, also run the relevant payload builder crate check.

## Non-Goals For This Stack

- Persisting the accumulator in a database table.
- Rebuilding the accumulator from canonical hashed state.
- Shadow-building before activation.
- Replacing header `state_root` validation.
- Disabling MPT or sparse-trie code globally.
- Defining new proof RPC behavior.
- Production checkpoint/snap-sync accumulator trust.

## Review Risks

- Old-state reads must use the parent state, not partially updated current
  state.
- A key must subtract its original element at most once.
- A key must add only its latest final element at finalization.
- Optimistic additions must be subtracted when superseded.
- BAL and non-BAL must feed the same replacement semantics.
- `EndBlock` must be a real barrier for all old reads.
- Account deletion with untouched nonzero storage must not be silently wrong.
- Sparse-trie BAL config flags must not disable Lthash.
