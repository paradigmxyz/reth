# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project Overview

Reth is an Ethereum execution client written in Rust. The workspace contains the
node binary and libraries for execution, storage, networking, and RPC.

## Commands

```bash
cargo build -p reth
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo nextest run --workspace
cargo docs --document-private-items
```

## Architecture

- `bin/reth/`: node binary.
- `crates/node/`, `crates/engine/`: node setup and consensus engine.
- `crates/evm/`, `crates/ethereum/`: execution and Ethereum-specific components.
- `crates/storage/`, `crates/trie/`, `crates/stages/`: persistence, state roots, and sync.
- `crates/net/`, `crates/rpc/`: P2P networking and JSON-RPC.

## Testing

- Add regression coverage in existing unit or integration tests.
- Reuse test helpers and isolate test databases and directories.
- Run affected packages first; broaden checks for changes across crates.
- Use existing benchmarks for performance changes and report measured results.

## Commit and PR Style

Use conventional commits and PR titles: `type: description`, with an optional
scope. Keep subjects under 50 characters where practical. Explain what changed
and why in one short paragraph. Link related issues and include real measurements
for performance claims; omit templates and validation boilerplate.

## Notes

- **Testing**: Use fuzz tests for parsing and serialization, and property tests for invariants.
- **Hot paths**: Avoid unnecessary allocations; reuse buffers and borrow where practical.
- **Logging and metrics**: Follow the component's existing `tracing` targets and metrics conventions.
- **Filesystem**: Use `reth_fs_util` for filesystem operations and its error context.
- **Async work**: Keep blocking work off executor threads; follow the component's task model.
- **Vendored code**: Do not edit `crates/storage/libmdbx-rs/mdbx-sys/libmdbx/`.
- **Type order**: Keep the file's primary type before supporting types and private helpers.
- **Documentation**: Update relevant docs when behavior or public APIs change. Regenerate CLI
  reference pages with `make update-book-cli` after changing commands or flags; do not edit
  generated pages by hand.
- **Dependencies**: Run `zepter run check` and `make lint-toml` for dependency changes.
- **PR labels**: Check available labels and apply those relevant to the change.

## Code Style

- Follow existing patterns and keep changes focused.
- Explain non-obvious behavior and safety requirements in comments, not PR history.
- Comments end with periods, except URLs. Document safety requirements of unsafe code.
- Files use LF and end with a newline.
- Never expose secrets.

### Rust

- Generally add new Rust functions, methods, `impl` blocks, modules, imports, Cargo dependencies, and other items at the bottom of the relevant scope, section, or group. Constructors usually go at the top of an `impl` block. First check where similar items sit in the file and match the existing order, grouping, and style, including alphabetical order where used.
- Put doc comments before attributes, always: `/// ...` comes before `#[derive]`, `#[inline]`, `#[cfg]`, and every other attribute.
- Put module documentation at the top of the module file with inner doc comments (`//! ...`), not on the `mod` item in the parent module.
- NEVER put imports inside functions unless required for `#[cfg(...)]` gating. All imports go at the top of the file.
- Group all `use` imports together. Keep `pub use` imports in a separate group. For local module re-exports, write `mod x;` before `pub use x;`; for re-exporting another module or external crate, use `use x;`, then a blank line, then `pub use y;`, then a blank line before local `mod my_mod; pub use my_mod::*;`.
- Put imports used only by tests inside the relevant `#[cfg(test)]` module, merging them into its ordinary imports instead of adding `#[cfg(test)] use` items to the parent. Keep any additional feature or platform conditions on those imports. Retain parent-level test-gated imports or re-exports only when test-only helpers or multiple test modules need them there.
- Keep crate-level dependency anchors such as `#[cfg(test)] use cc as _;` at crate scope.
- Put conditional imports after unconditional imports, separated by a blank line. Group imports with the same `#[cfg(...)]` condition together, with a blank line between different conditions. Apply this to every feature, platform, and test gate, including imports in nested modules. Within each group, merge imports from the same crate when their conditions and other attributes match. Keep the full condition on each `use`; do not introduce import-only modules or macros to avoid repeated attributes. Apply the same ordering within the separate `pub use` group, keeping local re-exports after their module declarations.
- In test modules, always import the parent module with `use super::*`.
- In `Cargo.toml`, generally group optional dependencies for a feature together. Put a comment immediately above the group containing only the feature name, for example `# jit`.
- Prefer `let Some(x) = x else { return };` / `let Ok(x) = x else { return };` over `match x { Some(x) => x, _ => return }`.
- Use `let ... else` only for a single early-exit guard. When multiple conditions or patterns gate the same block, prefer a combined `if let` / `let` chain instead of several sequential `let ... else` statements.
- Use combined `if let` chains (`if let Some(x) = x && let Some(y) = y { ... }`) instead of nesting (`if let Some(x) = x { if let Some(y) = y { ... } }`).
- In loops, prefer an `if let` chain around the loop body over multiple `let ... else { continue };` statements when the body only runs if all patterns match.
- NEVER use `ref` / `ref mut` in patterns as the first resort. Always prefer borrowing the expression with `&` / `&mut` instead.
- Prefer map entry APIs such as `entry`, `or_insert`, and `or_insert_with` when multiple consecutive operations would otherwise look up and then insert or update the same key.
- Avoid specifying type hints in variables unless absolutely necessary (e.g. `HashMap<_, Vec<_>>` for `x.entry(y).or_default().push(z)` where type inference won't work). Rely on the compiler.
- When type hints are needed, prefer turbofish (`let x = Type::<X, Y>::new()`) over annotation (`let x: Type<X, Y> = Type::new()`).
- In tests, avoid `.contains` assertions for error/output strings when the project has snapshot testing support such as `snapbox`. Prefer exact snapshot assertions (`stderr_eq`, `stdout_eq`, `assert_data_eq!`, etc.) and use redactions only for genuinely variable parts.
- Always leave a blank line in between module doc-comments, items or item categories, unless in rare exceptions: it's a one-shot struct with one single impl block, or it's a list of impls that are all very similar. But in general blank line in between items is the norm but it's just unenforced. Items includes imports (together) too. The previous rules apply first.
