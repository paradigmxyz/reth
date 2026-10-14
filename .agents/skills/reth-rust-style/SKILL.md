---
name: reth-rust-style
description: Review rules reth reviewers enforce beyond AGENTS.md. Use when writing, refactoring, or reviewing Rust in reth, before opening or updating a PR.
---

# Reth Rust Style

These are the comments reviewers leave most often. Follow them so they don't have to. AGENTS.md still applies; this file only adds to it.

## Scope

- Change only what the task needs. No drive-by fixes, renames, import reshuffles, formatting-only changes, or moved tests. Put unrelated work in its own PR.
- Before pushing, read the full diff and revert every hunk the PR description doesn't explain.
- Refactors keep behavior identical. If behavior must change, say so in the PR description.
- When wrapping or moving a type, keep existing call sites unchanged (`Deref` or delegating methods).

## Reuse before writing

- Before writing a helper, type, or constant, search the workspace and alloy and revm for one that exists.
- Prefer upstream APIs to manual byte handling:
  - `B256::from(U256)`, `U256::from(B256)`, `Address::into_word()`
  - `Log::topic0()`, `Signature::as_bytes()`, `Bytes::from([u8; N])`
  - `PayloadId::from_str`, alloy `From` impls between RPC and consensus types
  - `alloy_rlp::decode_exact`, `abi_decode_validate`, `sol!` for ABI types
- Use alloy's fixed-bytes maps (`B256Map`, `AddressMap`) for maps keyed by hashes or addresses.
- Use the constructors a type already has (`ExecOutput::done`, `ExecOutput::in_progress`, `StorageEntry::new`) instead of struct literals.
- When the same logic appears twice, extract one helper.

## Keep it small

- No helper, wrapper, trait, enum, or type alias with a single call site. Inline it. The one exception is a type alias that replaces `#[allow(clippy::type_complexity)]`.
- Inline single-use locals.
- Add code only with a caller in the same PR. This covers `pub` items, fields, accessors, config knobs, CLI flags, RPC methods, cargo features, generics, `PhantomData`, and extra bounds.
- No caches, interior mutability, or format versioning without a measured need.
- Don't re-check an invariant that is already enforced upstream. If correctness depends on it, say where it's enforced in a one-line comment.
- Keep one source of truth: no duplicate fields, flags, or counters for the same state.
- When removing or replacing something, delete everything it leaves behind: unused params (don't `_`-prefix them), dead variants and constants, unused Cargo dependencies, stale docs. Never leave commented-out code.

## Public API

- reth is a library. Don't remove or rename public items without a reason in the PR description.
- A new trait method needs a default or no-op impl unless every implementor must provide it.
- Public types need public fields or accessors for the data callers need.
- Destructive or write operations belong on the mutable transaction (`DbTxMut`), not on `DbTx`.
- Re-export types that appear in public signatures from the crate that exposes the signature.

## Comments and docs

- A comment states one non-obvious fact. Don't restate the code or narrate the change.
- Document new public traits, methods, and fields: what it does, when it is called, ordering guarantees, what `None`, `false`, or an error means, and side effects. Mark destructive operations as such.
- Keep existing comments and TODOs when moving code.
- When behavior changes, update everything that describes it in the same PR: doc comments, numbered step lists, `expect` and `unreachable!` messages, metric names.

## Naming

- Names say what a thing does or holds, including side effects (`*_blocking`).
- Match sibling names (`*_with_nodes` next to the trait that uses it).
- Rename an item when its meaning changes. Leave unrelated names alone.
- Don't reuse names of alloy types for different things (prefix with `Pooled*` or similar).
- Don't alias imports with `use X as Y`; it breaks grep.

## Errors

- No `unwrap` outside tests. `expect("why")` is only for true invariants; never `expect`, `panic!`, or `unreachable!` on a path that input, a peer, or the network can reach.
- Use typed errors. Never `Result<_, String>` or an early `.to_string()`; carry the source error in the variant. Use the error type the file already uses (`StaticFileWriterError`, `ProviderError`), and don't report provider errors as consensus errors.
- Never discard a fallible result with `.ok()` or `let _ =`. Ignore only the specific variant you expect.
- Don't turn values a spec leaves open into hard decode errors. Decode, then truncate or reject at the layer that owns the limit.
- Use `checked_*` and return an error for gas and index math. Use `try_from` for conversions; `as` only when it can't truncate.

## Types

- Make invalid states unrepresentable. Put variant-specific data inside enum variants, not in correlated `bool`/`Option` fields.
- Use a named struct or newtype instead of a tuple (`.0`, `.1`) or `Option<u8>` plus casts in public or serialized types.
- Use named constants (spec constants, `http::StatusCode`) instead of literals, and `usize` for limits that size collections.
- Bump the version constant whenever a persisted or serialized shape changes, and check serde compatibility of changed public types.
- A function that doesn't use `self` is a free function. Logic that belongs to a type goes in its `impl`.
- Reject mutually exclusive CLI flags at parse time with clap's `conflicts_with`, and update the parse-all test.

## Tests

- Every new branch, guard, error path, and unwind or reopen path gets a test in the same PR.
- Assert the exact error or message, not just `is_err()`.
- Fixtures must be coherent: request the same root the fixture verifies, and cover every match arm the loop claims to.
- Before deleting a test as redundant, show that another test fails when the line it guards breaks.
- Don't commit `#[ignore]` manual perf tests. Write a bench.

## Async and observability

- Every wait is bounded and observes shutdown. Every queue and retained map has a bound.
- Spawned tasks enter the caller's span for the whole closure. Check that the span survives an `INFO` filter.
- Use `error!` for local faults and `warn!` for peer or external faults. Log `info` once in wait or poll loops, then `debug`.
- Log `debug!` or `trace!` on state transitions in background tasks.
- Count skips with a dedicated metric, not by recording zeros.

## Layout and visibility

- Use names already imported (`b256!`, `B256`) instead of fully qualified paths, and remove the whole import line when dropping one.
- Keep items `pub(crate)` or private unless another crate uses them.
- Use a type alias instead of `#[allow(clippy::type_complexity)]`, and `#[expect(lint, reason = "...")]` instead of `#[allow]`.

## Performance

- Don't clone large values to read them. Borrow, `take`, or share through `Arc`. Getters return references.
- Don't read the same DB value twice, and build optional data lazily (`Option`) when only some paths need it.
- Cap any length that comes from untrusted input before `Vec::with_capacity` or a loop.

## Dependencies and CI

- Workspace dependencies have default features off. Check that each touched crate builds on its own, with and without its features.
- Keep `no_std` crates free of `std`; use `core` and `alloc`.
- Dependency bumps respect the MSRV.
- Pin reusable workflows by SHA only after the 7-day soak, with a trailing `# repo#PR` comment. Never reference `@main` or expose secrets to cargo builds.
