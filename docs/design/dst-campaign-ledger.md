# DST campaign defect ledger

Working branch: `centaur/extend-dst-properties-1791211405`, based on
`centaur/rebase-reth-dst-main-1790871200` at `5fbd646697`.

## Reproduced runtime defects

| ID | Classification | Reproduction and evidence | Resolution |
| --- | --- | --- | --- |
| DST-1 | Cooperative execution deadlock | Before the fix, node seeds 0 and 1 timed out at payload import after 90 and 30 host seconds, respectively; traces `node-0-7094c649.dst` and `node-1-34e2eea6.dst` stopped with sparse-trie and prewarm jobs runnable. | A synchronous hashed-state receive on the simulator executor could wait for a cooperative worker that required that same executor. Compute the hash inline on the sequential/cooperative lane; seeds 0 and 1 then completed. |

## Campaign/harness failures (not counted as product defects)

| ID | Evidence | Resolution |
| --- | --- | --- |
| H-1 | Seed 15, trace `node-15-8663dfb0.dst`: an empty payload triggered `txpool prewarming did not publish a snapshot`. | No sender needs warming for zero transactions, so the campaign must not wait for a snapshot. Seed 15 completed after this adjustment. |
| H-2 | The starting branch did not compile with current engine-tree interfaces (28 errors, then 5 runner errors). | Reconciled the native/cooperative sparse-trie, BAL, provider, receipt, and Engine-message APIs before starting the campaign. These are build blockers, not independently reproduced runtime bugs. |
| H-3 | After adding slot snapshots, 11 of seeds 0–15 failed on `None` versus `Some(0)` for untouched or cleared storage. | State-provider representations differ but both mean an EVM slot value of zero; compare `unwrap_or_default()` and continue comparing nonzero values, code, and receipts. |
| H-4 | Seed 89 repeatedly returned `MissingPayload` when the campaign moved fork choice away from an in-progress payload build and then polled its old payload ID. | Treat `MissingPayload` as a terminal canceled build, remove its pending pool transactions, and assert fork choice did not change. Do not count a canceled job as a successful build or a product bug. |
| H-5 | Native-worker seeds 0–3 returned valid payloads without a preserved sparse trie. | Native validation can finish with a serial-root fallback, leaving no reusable trie. Require preservation in the cooperative lane; if a native trie is present, still check its block hash. |

## Qualification record

- `cargo check -p reth-dst-runner --features dst --bin reth-dst-node`: passed (one unrelated unused-variable warning).
- Node seeds 0–15 passed with the expanded code/storage/receipt comparison. Seed 89 passed after canceled payload jobs were modeled. The uninterrupted qualification from seed 90 ran for 1,860 seconds (31 minutes) at 32 actions per case: all 332 seeds 90–421 passed with no new distinct failure. The last unique reproduced product defect remains DST-1; the 10-defect discovery goal was not reached.
- Deep-history runs passed seeds 2000–2009 at 128 actions and seeds 3000–3019 at 256 actions, including multi-error recovery. Decision bound for the deep runs was 65,536.
- Native-worker differential seeds 0–23 passed (0–3 at 16 actions; 4–23 at 32 actions), as did seeds 100–103 at 256 actions, with `--features dst,native-differential` and `RETH_DST_NATIVE_WORKERS=1`. This does not simulate a real process crash or power loss, and the native mode does not inject database faults.
- Node seeds 0 (injected commit failure) and 1 also passed strict semantic replay with `RETH_DST_VERIFY_PASSES=1` at 16 actions per case.
- Nightly Clippy passed with warnings. Dependency checks `zepter run check` and `make lint-toml` could not run because `zepter` and `dprint` are not installed in this sandbox.
- `RETH_DST_SEED=0 RETH_DST_CASES=1000 RETH_DST_STEPS=64 ./target/debug/reth-dst-witness`: seeds 0–999 passed, both witness reconstruction and adversarial proof verification.

The component witness run does not cover process crashes, filesystem durability, native thread
preemption, or power-loss behavior. A simulator timeout without a confirmed root cause is
inconclusive, not a product bug. Repeated failing seeds that share a root cause count once.
