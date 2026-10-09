# TIP-1143 storage interface

This is an opt-in storage facility. It does not activate TIP-1143, change Ethereum
creation limits, reinterpret opaque account extensions, or migrate existing records.
Explicit typed account metadata opts new outcomes into chunk publication.

## Public boundary

`reth_storage_api` exports `ValidatedCode`, `CodeChunkDescriptor`,
`CodeRepresentation`, `CodeReadContext`, and `CodeChunkReader` (an alias of
`BytecodeReader`). The constants are `CODE_CHUNK_SIZE = 24541`,
`MAX_CODE_CHUNKS = 40`, and `MAX_CODE_SIZE = 981640`.

`ValidatedCode::new(Bytes)` keeps original logical bytes and hashes unchanged. It slices
at 24,541 bytes and scans PUSH and the immediate-bearing EIP-8024 instructions.
It derives bounded leading-data lengths, up to 32 original lookahead bytes,
and the next-chunk index or final STOP. Boundary STOPs are not required. The EVM2
adapter creates execution buffers with valid leading replacement JUMPDESTs and
private transitions. Each legal entry offset derives its own stack-neutral transition
so jumps into EIP-8024 operands retain the established jump-map behavior. This intentionally permits jumps into replaced leading immediate
positions; it does not claim complete equivalence to unmodified-code execution.
`ValidatedCode::from_chunks` authenticates raw sizes and hashes before deriving this
context. Unchanged historical single records remain valid through 24,576 bytes.

`CodeChunkDescriptor::new(u32, Vec<B256>)` validates a multi-chunk size and exact
ordered hash count. `code_size()`, `chunk_hashes()`, and `chunk_range(u32)` expose
checked bounds. It is independent of Tempo's extension encoding.

The provider operations take `&self`:

```rust,ignore
fn get_code_chunk_by_hash(&self, hash: &B256, index: u32)
    -> ProviderResult<Option<Bytes>>;
fn get_required_code_chunk(&self, hash: &B256,
    representation: &CodeRepresentation, index: u32)
    -> ProviderResult<Option<Bytes>>;
fn get_required_code_chunk_with_context(&self, hash: &B256,
    representation: &CodeRepresentation, index: u32,
    context: Option<CodeReadContext>) -> ProviderResult<Option<Bytes>>;
fn bytecode_by_hash(&self, hash: &B256)
    -> ProviderResult<Option<reth_primitives_traits::Bytecode>>;
```

Hash-only discovery may read a descriptor before determining representation.
Unknown hashes return `None`. Required reads use the representation decoded by
Tempo from the account belonging to the requested state view: `Empty`, `Legacy`,
or `Chunked(descriptor)`. Empty and known invalid indices return `None` before
storage. Missing required legacy zero or required descriptors are errors. Stored
and committed descriptors must agree. In-range reads fetch exactly one payload;
missing or wrong-length payloads error even if a whole-code row exists.
Legacy zero reads the unchanged `Bytecodes` record. Oversized legacy records are
not silently split: sparse zero errors, while full-code access remains unchanged.

`StateProvider::account_code(&Address)` requires full code when the surviving
account has a nonempty code hash. If hash discovery returns `None`, it returns
`CodeChunkErrorKind::MissingCode`, with index zero, unknown expected length, and
no execution context. This includes descriptor loss when no whole-code row exists.
It does not infer representation or size from opaque extension bytes. Missing
accounts, absent code hashes, and `KECCAK_EMPTY` still return `None`. Existing
RPC `get_code` propagates this required-data error through its public internal
error mapping rather than reporting empty bytes. Payload/reconstruction errors
retain their original structured fields.

`ProviderError::CodeChunk` carries hash, index, optional expected length, exact
reason, original database cause when applicable, and optional block/transaction
hashes. Context-free reads set context to `None`. Failures are not cached as
absence. Retry against a fresh database snapshot after repair.

## Persistence and publication

MDBX tables are:

- `BytecodeChunkDescriptors`: full code hash -> version byte 1, big-endian u32
  original length, then per chunk: 32-byte hash, leading-data length byte, original jump-analysis leading-data byte,
  lookahead-length byte, next-chunk byte (255 means STOP), and lookahead bytes.
- `BytecodeChunks`: payload hash -> original payload bytes.
- `Bytecodes`: unchanged legacy values and bytecode-kind encoding.

Descriptors contain only bounded lookahead, never complete payloads. Preparation is keyed
by global code identity, because shared raw payloads can have different contexts.
Descriptor decoding validates framing and
bounds before allocating hashes. Reconstruction loads ordered payloads, verifies
each hash and derived length, verifies the concatenation's full hash and recomputed preparation, and returns
explicitly analyzed legacy bytecode. EF01 prefixes cannot change multi-chunk kind.
Existing legacy rows return their stored kind, including genuine delegation.
Sparse reads trust authenticated immutable payloads and do not rehash on access. The execution-only legacy getter
reads the historical record directly when old and chunked accounts share a hash;
public reconstruction still checks known chunk descriptors and required payloads.

`DatabaseProviderRW::write_chunked_code(address, account, &ValidatedCode)` checks
the account's code hash before any writes. `import_chunked_code(address, account,
size, hashes, payloads)` authenticates untrusted input against the account hash
before invoking the same publication path. The caller composes all extension
bytes; Reth preserves them unchanged. Replacing/clearing code requires the caller
to supply the corresponding replacement account extension.

Both storage settings use **one MDBX write transaction** for this operation:
v1 publishes to `PlainAccountState`; v2 publishes to `HashedAccounts`. Descriptors
and payloads remain in MDBX in both settings. This is not a cross-database atomic
commit guarantee for arbitrary other work mixed into the provider. On storage
failure the publication API poisons provider commit; discard the writer. Do not
extract and manually commit its underlying transaction after an error.

Immutable content remains available for other owners and historical account
views. There is no code garbage collection or activation backfill. Whole database
copies include both tables. `DbTool::drop_table` refuses separate chunk-table
clears while descriptors remain, including content retained for historical state.

## Adapters and synchronization

Latest and historical/overlay providers use transaction-backed readers. Resident
overlay code and resident cache/snapshot code can supply selected original slices
without a backing full-code miss loader. The execution cache forwards other
chunk requests and retains its existing full-code cache behavior. `EvmStateProvider`
uses the `BytecodeReader` contract; reference, Arc, Box, erased, execution database,
and instrumentation adapters preserve it. Providers lacking sparse support return
`UnsupportedProvider`; the RPC-backed provider does not download full code to
implement sparse access. Existing RPC and snap serving use full-code reconstruction.

`SnapBytecodeStore::stage_chunked_code(write, hash, size, hashes, payloads)`
authenticates immutable content within an authorized snap attempt without
publishing an account. The caller must abort after a storage write failure.
Account publication and completeness checks recheck actual required content;
a descriptor alone is insufficient. Ordinary downloaded bytecode remains unsplit.
`VerifiedAccountRange::verify_response(request, response)` invokes the downloader's
existing proof-validation path, without exposing an unchecked range constructor.

## Observations and coordinated bridge

`provider.code_chunks.read_latency` samples each attempted multi-chunk payload
lookup, including missing/malformed payload results. `bytes_fetched` counts actual
returned payload bytes before length validation. Descriptor lookups and invalid
indices are excluded. `execution_cache.code_chunks.hits/misses` counts successful
in-range chunk requests served by resident content or the backing provider.
These aggregate metrics carry no address/hash labels and are not disk latency or
logical EVM warmth measurements. Execution owns logical cold-access counters.

`StateProviderDatabase` implements the required EVM2 sparse chunk operation. It
validates account size and hashes against the persisted descriptor, then returns
only the selected original payload together with its authenticated preparation.
`CodeChunk::with_preparation` receives the global size, index, both leading-data
counts, and original lookahead. EVM2 derives exact prepared views for each allowed
entry offset. Legacy records retain their stored kind through
`CodeChunk::from_bytecode`; no individual payload prefix selects delegation.

Reth's cache wrappers forward contextual reads and `discard_code_chunk`. Sparse
readers do not retain rejected raw responses. A real MDBX cold-reopen integration
test publishes a forty-chunk contract through the normal outcome path, executes
it through `StateProviderDatabase`, and observes only the entry and remote chunk
payload reads, with no full `Bytecodes` lookup. The test covers both storage
versions. The assembled integration workspace must pin all three repository
revisions and patched dependencies before release.

Network legacy-code inventory, unchanged-database provenance review, differential
replay, parser fuzz campaigns, cold database latency, proof/cache growth,
transaction gas measurements, tariffs, and activation approval remain pending.
No production measurements or activation readiness are claimed.

## Typed account metadata and normal outcomes

The patched account-extension dependencies distinguish a typed RLP list from the
legacy opaque fifth-field string. Arbitrary opaque bytes, including bytes resembling
the new encoding, retain their old meaning. Compact records have a separately framed
typed trailer; bytes inside a legacy opaque payload never select the new format.
The typed payload is tag 1 + u32 size + count + ordered hashes for chunked runtime,
or tag 2 + 20-byte target for inline delegation. The existing account hash authenticates
the exact delegation marker. Reth's checked conversion rejects malformed metadata.

EVM2/revm outcome conversions retain unrelated opaque fields and code metadata.
Normal `write_state` and `write_state_changes`, including the storage-v2 fast path,
select chunk publication only from explicit account metadata. Historical or disabled
outcomes continue writing their unchanged full bytecode records. Existing legacy and
chunked accounts can share a global hash without deleting the historical record.

New inline accounts resolve without reading any code table. Ordinary account metadata
reads do not touch code tables either. The required EVM2 `get_code_kind_by_hash`
operation uses only checked Compact framing and the stored kind tag. `account_code` synthesizes
their marker for RPC. Old-format accounts use a bounded Compact framing/kind lookup;
ordinary runtime payloads are not cloned or analyzed by that lookup, and old delegation
remains an old-format account until an explicit code change. The runtime requests
its exact marker only after kind resolution, before reserving target chunk gas. This compatibility lookup
is metadata I/O, not a claim of zero disk access for historical accounts.

Snap account publication and completion verify the typed account commitment, accept
inline delegation without a marker payload, and derive prepared storage from supplied
full code when needed. Staging uses the same poison-on-storage-error publisher as
normal provider code: a partially failed chunk write cannot be committed.
