# BAL Async Sparse Trie Prototype

This document sketches a BAL-only async sparse trie prototype for the payload processor.

The goal is to move trie ownership into per-trie actors while keeping the current arena sparse
trie logic mostly intact. The first prototype should preserve correctness and cache reuse before
trying to replace internal rayon parallelism.

## Scope

This is a BAL-specialized prototype.

For the first version, every `StateRootMessage::HashedStateUpdate` is expected to be one-account
scoped:

```text
account-only: accounts.len() == 1 && storages.is_empty()
storage-only: accounts.is_empty() && storages.len() == 1
```

The final hashed-state side channel can be treated as a TODO in the initial prototype. If it is
not implemented, the sender must be closed or otherwise handled so callers fall back to recomputing
hashed state instead of blocking.

## Architecture

```text
StateRootMessage
      |
      v
AsyncSparseTrieCoordinator
  |          |              |
  |          |              v
  |          |        ProofService
  |          |              |
  |          |              v
  |          |        ProofWorkerHandle
  |          |
  |          v
  |    AccountTrieActor
  |
  v
StorageTrieActor(address)
```

The coordinator owns orchestration and cache policy. Trie actors own trie mutation.

## Coordinator

The coordinator owns:

- `SparseStateTrie` container
- LFU/cache policy
- pending account metadata
- storage roots
- actor registry
- proof service handle
- finalization and preservation

It does not RLP-encode leaf values and does not mutate account or storage tries directly.

### Startup

1. Take the preserved sparse trie.
2. Convert it with `into_trie_for(parent_state_root)`.
3. Call `set_hot_cache_capacities(max_hot_slots, max_hot_accounts)`.
4. Take the account trie from `SparseStateTrie` and spawn `AccountTrieActor`.
5. Spawn `ProofService`.
6. Spawn storage actors lazily on first storage update for an address.

Storage trie extraction already has suitable APIs:

```text
take_or_create_storage_trie(address)
insert_storage_trie(address, trie)
```

Account trie extraction likely needs a small API addition:

```text
take_accounts_trie()
set_accounts_trie(trie) // already exists
```

### BAL Message Routing

Storage-only update:

```text
Coordinator:
  assert one storage entry
  record_slot_touch(address, slot) for each slot
  spawn StorageTrieActor(address) if needed
  send StorageTrieCmd::ApplyHashedStorage(storage)
```

The storage actor performs storage value RLP encoding:

```text
for (slot, value):
  if value == 0 -> LeafUpdate::Changed(Vec::new())
  else -> LeafUpdate::Changed(rlp(value))
```

Account-only update:

```text
Coordinator:
  assert one account entry
  record_account_touch(address)
  pending_accounts[address] = account
  send AccountTrieCmd::Touch(address)
```

Touching the account trie lets account proof/reveal begin early. Final account leaf encoding is
delayed until all storage roots are known.

## StorageTrieActor

Each storage actor owns one `RevealableSparseTrie<S>`.

Actor state:

```text
address
trie: RevealableSparseTrie<S>
ready_updates: B256Map<LeafUpdate>
blocked_updates: B256Map<LeafUpdate>
root: Option<B256>
deferred_drops
```

Commands:

```text
ApplyHashedStorage(HashedStorage)
Reveal(Vec<ProofTrieNodeV2>)
ReturnTrie
Shutdown
```

Events:

```text
ProofTargets { address, targets }
RootReady { address, root }
ReturnedTrie { address, trie, deferred_drops }
Error
```

Loop shape:

```text
loop:
  drain queued commands with try_recv

  if ready_updates not empty:
      drive_once()
      yield
      continue

  wait for next command
```

`drive_once()` is synchronous and bounded:

```text
take bounded update chunk
trie.update_leaves(chunk, collect proof targets)
send proof targets to ProofService nonblocking
move leftovers from chunk to blocked_updates

if ready_updates empty && blocked_updates empty:
    root = trie.root()
    emit RootReady
```

On reveal:

```text
trie.reveal_nodes(nodes)
defer dropped proof node vec
move blocked_updates -> ready_updates
root = None
```

Storage root is computed eagerly once that storage trie drains. This avoids a final fan-out root
calculation at `FinishedStateUpdates`.

## AccountTrieActor

The account actor owns the account trie and account RLP buffer.

Commands:

```text
Touch(Vec<B256>)
Reveal(Vec<ProofTrieNodeV2>)
FinalizeAccounts {
  pending_accounts: B256Map<Option<Account>>,
  storage_roots: B256Map<B256>,
}
RootWithUpdates
ReturnTrie
Shutdown
```

Events:

```text
ProofTargets(Vec<ProofV2Target>)
Finalized
RootReady { root, updates }
ReturnedTrie { trie, deferred_drops }
Error
```

`Touch` applies `LeafUpdate::Touched` for account paths and emits proof targets if blinded.

`FinalizeAccounts` builds final account leaf updates:

```text
for each pending account:
  storage_root = storage_roots.get(address)
      or existing account leaf storage_root
      or EMPTY_ROOT_HASH

  encoded = encode_account_leaf_value(account, storage_root)
  account_updates[address] = LeafUpdate::Changed(encoded)
```

Then it applies updates with the same bounded `drive_once` pattern as storage actors.

## ProofService

The first prototype should use the least-change proof design: a central proof service that reuses
the existing account multiproof path.

The service owns:

```text
ProofWorkerHandle
pending_targets: MultiProofTargetsV2
fetched_account_targets: B256Map<u8>
fetched_storage_targets: B256Map<B256Map<u8>>
inflight_proofs
actor routing table
```

Inputs:

```text
ProofRequest::Account(Vec<ProofV2Target>)
ProofRequest::Storage { address, targets: Vec<ProofV2Target> }
ProofRequest::Flush
ProofRequest::RegisterStorageActor { address, tx }
```

`Flush` is not periodic. It is sent when an actor has emitted targets and is now blocked/idle, and
during final drain. The service also dispatches automatically when target count crosses chunking
thresholds.

Dispatch reuses existing machinery:

```text
dispatch_with_chunking(...)
ProofWorkerHandle::dispatch_account_multiproof(...)
```

Proof results are `DecodedMultiProofV2`:

```text
account_proofs -> AccountTrieCmd::Reveal(account_proofs)
storage_proofs[address] -> StorageTrieCmd::Reveal(nodes)
```

No direct storage-proof API is required for the prototype.

## Finalization

On `FinishedStateUpdates`:

```text
Coordinator:
  mark input finished
  flush ProofService
  wait until all touched storage actors emit RootReady
  wait until ProofService inflight == 0
  send FinalizeAccounts to account actor
  flush ProofService as needed
  wait until account actor finalized and root ready
  collect account root and trie updates
  ask all actors to ReturnTrie
  reinsert tries into SparseStateTrie
  assemble TrieUpdates
  send StateRootComputeOutcome
```

## Reuse And Pruning

For the prototype, pruning stays centralized and uses the existing `SparseStateTrie` behavior.

After actors return tries:

```text
coordinator reinserts account/storage tries into SparseStateTrie
coordinator uses existing into_trie_for_reuse flow:
  commit_updates(&trie_updates)
  prune(max_hot_slots, max_hot_accounts)
  shrink_to(max_nodes_capacity, max_values_capacity)
  take_deferred_drops()
  store PreservedSparseTrie::anchored(trie, state_root)
```

On failure or cancellation:

```text
return actors' tries if possible
clear SparseStateTrie
shrink
store PreservedSparseTrie::cleared(trie)
```

## Invariants

- Only the owning actor mutates its trie.
- The coordinator never RLP-encodes leaf values.
- The coordinator owns LFU and cache preservation.
- Storage actors compute roots eagerly after drain.
- Account finalization happens only after `FinishedStateUpdates`.
- Proof targets are flushed before an actor waits for proof reveal.
- No `.await` while arena subtries are taken.
- Arena internals keep current rayon parallelism initially.
- One storage actor per touched storage trie is acceptable for the prototype.

## Implementation Phases

### Phase 1: Small API Prep

Add minimal sparse trie container APIs:

```text
SparseStateTrie::take_accounts_trie()
SparseStateTrie::insert_accounts_trie(...)
```

Optionally add:

```text
DeferredDrops::extend(...)
```

Add focused unit tests for taking/reinserting account and storage tries without losing update
tracking.

### Phase 2: ProofService

Implement the central proof service while keeping the current `SparseTrieCacheTask` unchanged.

It should reuse:

```text
PendingTargets-style accumulator
dispatch_with_chunking
ProofWorkerHandle::dispatch_account_multiproof
DecodedMultiProofV2 routing
```

Test with mocked actor senders: submit account/storage targets, feed a proof result, and assert
account proofs and per-address storage proofs route correctly.

### Phase 3: StorageTrieActor

Implement one storage actor owning one `RevealableSparseTrie`.

Support:

```text
ApplyHashedStorage
Reveal
ReturnTrie
```

Emit:

```text
ProofTargets
RootReady
ReturnedTrie
```

Use existing `update_leaves`; keep rayon unchanged. Add tests with a small trie harness: apply
update, receive proof target, reveal proof, drain, compute root.

Also test the large-batch path with chunked `drive_once`.

### Phase 4: AccountTrieActor

Implement the account actor with:

```text
Touch
Reveal
FinalizeAccounts
RootWithUpdates
ReturnTrie
```

Use account leaf encoding inside the actor. Test:

```text
Touch on blinded account emits proof target
Finalize with storage root encodes account correctly
Finalize storage-only account preserves existing account fields
Deletion encodes empty leaf when account empty and storage root empty
```

### Phase 5: BAL Coordinator Prototype

Wire the async coordinator for BAL-only input:

```text
StateRootMessage::HashedStateUpdate -> route to account/storage actor
StateRootMessage::FinishedStateUpdates -> finalization
```

Assert account-only XOR storage-only updates.

For the first prototype, make the final hashed-state side channel an explicit TODO and ensure the
sender cannot block validation.

### Phase 6: Reuse And Preservation

Add actor `ReturnTrie` flow and reassemble `SparseStateTrie`.

Then reuse existing centralized preservation:

```text
commit_updates
prune
shrink
store anchored/cleared
spawn_drop(deferred)
```

This phase tests LFU correctness. Touches recorded by the coordinator should affect retained
account/storage paths after prune.

### Phase 7: Integration Switch

Add an integration branch in `spawn_sparse_trie_task`:

```text
if async_bal_sparse_trie_enabled && BAL mode:
    spawn async prototype
else:
    current SparseTrieCacheTask
```

The prototype is enabled by default for BAL mode. `RETH_ASYNC_BAL_SPARSE_TRIE=0` disables it for
rollback, and `RETH_ASYNC_SPARSE_TRIE_THREADS` controls the persistent runtime worker count
(default 32).

### Phase 8: End-To-End Tests

Add BAL-specific tests:

```text
single account storage update
account and storage update for same address, account message first
account and storage update for same address, storage message first
large storage batch
multiple storage actors
empty/no-op BAL
```

Compare state root and trie updates against the current sparse trie task or serial state root.

### Phase 9: Metrics And Cleanup

Add metrics before performance work:

```text
storage actors spawned
actor active/idle time
proof targets emitted/dispatched
proof service pending/inflight
storage roots computed eagerly
large batch chunk count
reuse/prune time
```

Only after this phase should arena internal rayon be considered for replacement with joins on the
dedicated runtime. The actor architecture should work first.
