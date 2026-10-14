---
name: reth-rust-style
description: Review rules reth reviewers enforce beyond AGENTS.md. Use when writing, refactoring, or reviewing Rust in reth, before opening or updating a PR.
---

# Reth Rust Style

These are review comments reth reviewers leave often that AGENTS.md doesn't spell out. AGENTS.md still applies; this file only adds to it.

## Scope

- Before pushing, read the full diff and revert every hunk the PR description doesn't explain.
- Refactors keep behavior identical. If behavior must change, say so in the PR description.
- When wrapping or moving a type, keep existing call sites unchanged (`Deref` or delegating methods).

## Reuse before writing

- Prefer upstream APIs to manual byte handling:
  - `B256::from(U256)`, `U256::from(B256)`, `Address::into_word()`
  - `Log::topic0()`, `Signature::as_bytes()`, `Bytes::from([u8; N])`
  - `PayloadId::from_str`, alloy `From` impls between RPC and consensus types
  - `alloy_rlp::decode_exact`, `abi_decode_validate`, `sol!` for ABI types
- Use alloy's fixed-bytes maps (`B256Map`, `AddressMap`) for maps keyed by hashes or addresses.
- Use the constructors a type already has (`ExecOutput::done`, `ExecOutput::in_progress`, `StorageEntry::new`) instead of struct literals.
- When the same logic appears twice, extract one helper.

## Keep it small

- No wrapper, trait, enum, or type alias with a single use. The one exception is a type alias that replaces `#[allow(clippy::type_complexity)]`.
- No caches, interior mutability, or format versioning without a measured need.
- Don't re-check an invariant that is already enforced upstream. If correctness depends on it, say where it's enforced in a one-line comment.
- Remove unused parameters instead of `_`-prefixing them.

## Public API

- reth is a library. Don't remove or rename public items without a reason in the PR description.
- A new trait method needs a default or no-op impl unless every implementor must provide it.
- Public types need public fields or accessors for the data callers need.
- Destructive or write operations belong on the mutable transaction (`DbTxMut`), not on `DbTx`. Say in their docs that they are destructive.
- Re-export types that appear in public signatures from the crate that exposes the signature.

## Comments and docs

- A comment states one non-obvious fact. Don't restate the code or narrate the change.
- Keep existing comments and TODOs when moving code.
- When behavior changes, update everything that describes it in the same PR: doc comments, numbered step lists, `expect` and `unreachable!` messages, metric names.

## Naming

- Names say what a thing does or holds, including side effects (`*_blocking`).
- Match sibling names (`*_with_nodes` next to the trait that uses it).
- Rename an item when its meaning changes. Leave unrelated names alone.
- Don't reuse names of alloy types for different things (prefix with `Pooled*` or similar).
- Don't alias imports with `use X as Y`; it breaks grep.

## Errors

- Carry the source error in the variant, and don't report provider errors as consensus errors.
- Never discard a fallible result with `.ok()` or `let _ =`. Ignore only the specific variant you expect.
- Don't turn values a spec leaves open into hard decode errors. Decode, then truncate or reject at the layer that owns the limit.
- Use `checked_*` and return an error for gas and index math. Use `try_from` for conversions; `as` only when it can't truncate.

## Types

- Make invalid states unrepresentable. Put variant-specific data inside enum variants, not in correlated `bool`/`Option` fields.
- Use `usize` for limits that size collections, so they need no casts.
- Check serde compatibility of changed public types.
- A function that doesn't use `self` is a free function. Logic that belongs to a type goes in its `impl`.
- Reject mutually exclusive CLI flags at parse time with clap's `conflicts_with`, and update the parse-all test.

## Tests

- Every new branch, guard, error path, and unwind or reopen path gets a test in the same PR.
- Assert the exact error or message, not just `is_err()`.
- Fixtures must be coherent: request the same root the fixture verifies, and cover every match arm the loop claims to.
- Don't commit `#[ignore]` manual perf tests. Write a bench.

## Async and observability

- Every wait is bounded and observes shutdown. Every queue and retained map has a bound.
- A spawned task enters the caller's span for the whole closure, not only as the parent of a `DEBUG` child; check it survives an `INFO` filter.
- Log `debug!` or `trace!` on state transitions in background tasks.
- Count skips with a dedicated metric, not by recording zeros.

## Layout and visibility

- When dropping an import, remove the whole line and any blank line it leaves.
- Keep items `pub(crate)` or private unless another crate uses them.
- Use `#[expect(lint, reason = "...")]` instead of `#[allow]`.

## Performance

- Don't clone large values to read them. Borrow, `take`, or share through `Arc`. Getters return references.
- Don't read the same DB value twice, and build optional data lazily (`Option`) when only some paths need it.
- Cap any length that comes from untrusted input before `Vec::with_capacity` or a loop.

## Dependencies and CI

- Workspace dependencies have default features off. Check that each touched crate builds on its own, with and without its features.
- Keep `no_std` crates free of `std`; use `core` and `alloc`.
- Dependency bumps respect the MSRV.
- Pin reusable workflows by SHA only after the 7-day soak, with a trailing `# repo#PR` comment. Never reference `@main` or expose secrets to cargo builds.
