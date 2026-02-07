# LLM Invariant-Driven Development

## What is this?

A system for declaring, tracking, and enforcing codebase invariants using LLMs.

Traditional approaches to maintaining invariants rely on `debug_assert!`, doc
comments, and tribal knowledge. This system makes invariants **first-class,
machine-readable, and continuously enforced**.

## How it works

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ INVARIANTS.md│────▶│  LLM Agent   │────▶│  PR Review   │
│ (per crate)  │     │ (CI check)   │     │  (approve /  │
│              │     │              │     │   request    │
│              │     │ + diff       │     │   changes)   │
└──────────────┘     └──────────────┘     └──────────────┘
       ▲                    │
       │                    ▼
       │             ┌──────────────┐
       └─────────────│  Propose new │
                     │  invariants  │
                     └──────────────┘
```

### 1. Declare invariants

Add an `INVARIANTS.md` next to the code it covers (e.g., `crates/trie/common/INVARIANTS.md`).

Each invariant has:
- **ID**: `INV-<MODULE>-<NNN>` — stable identifier for cross-referencing
- **Scope**: file or module path the invariant applies to
- **Severity**: `critical` (blocks merge), `high` (blocks merge), `medium` (warning)
- **Statement**: plain-english rule that must always hold

### 2. CI enforcement

On every PR, an LLM agent:
1. Finds all `INVARIANTS.md` files in the repo
2. Identifies which invariants are relevant to the changed files
3. Reads the diff and reasons about whether any invariant is violated
4. Posts a review: approve, warn, or request changes

### 3. Invariant discovery

The agent also proposes new invariants it infers from:
- `debug_assert!` statements in changed code
- Doc comments describing constraints
- Test assertions that encode implicit rules
- Patterns it recognizes from the diff context

Proposed invariants go through human review before being added.

### 4. Test generation

Each invariant can have companion property-based tests auto-generated.
The agent translates the plain-english statement into a `proptest` harness.

## Adding invariants to a new crate

1. Create `INVARIANTS.md` in the crate root (next to `Cargo.toml`)
2. Follow the format in `crates/trie/common/INVARIANTS.md`
3. Use `critical` severity sparingly — only for invariants whose violation
   would cause data corruption or consensus failures

## Updating invariants

If a PR intentionally changes behavior that violates an invariant:
1. Update the invariant in the same PR
2. Add a `Reason` note explaining why the change is safe
3. The LLM agent will verify the invariant update is consistent with the diff

## FAQ

**Q: Does this replace unit tests?**
No. Invariants complement tests. Tests verify specific cases; invariants
declare universal rules that the LLM checks against arbitrary diffs.

**Q: What if the LLM gets it wrong?**
The agent's review is advisory for `medium` severity. For `critical`/`high`,
a human maintainer can override. False positive rate improves as invariant
statements get more precise.

**Q: How is this different from clippy lints?**
Clippy checks syntactic patterns. Invariants check semantic properties —
"does this diff preserve the guarantee that sorted output stays sorted?"
is not expressible as a lint rule.
