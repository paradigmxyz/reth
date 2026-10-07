# Experimental prewarm result handoff

Local base: `1e8eb0c1820276f387ac4edff41beb48f76120d9` in `paradigmxyz/reth`.

This experimental implementation reuses block-local transaction prewarming results during serial payload
validation. It is disabled by default. Enable it programmatically with
`TreeConfig::default().with_prewarm_handoff(true)` or use the hidden experimental
`--engine.prewarm-handoff` flag for controlled benchmark runs.

Workers record account metadata, original storage values, and block-hash reads while running normal
nonce and balance validation against the parent state. The canonical execution loop consumes only
already-ready results, checks the block's remaining gas, verifies every recorded dependency, and
uses the ordinary block executor to commit state and build receipts. A missing result, conflict,
provider error, oversized read set, or failed speculation falls back to normal serial execution.
System calls and other pre/post-execution changes stay on the existing canonical path.
Transactions with later sender nonces or funding dependencies fail strict parent-state speculation:
they are neither reused nor retried with permissive cache-only warming in this prototype.

An account loaded only after the outermost execution frame returns can be identified as the
beneficiary's fee-only access. Its reward delta is rebased onto the current balance, avoiding a
false conflict with earlier transaction fees. Nonce, code hash, and existence must still match.
Beneficiary accesses during transaction validation or EVM execution are exact dependencies. A
decreasing beneficiary balance or overflowing arithmetic conservatively rejects the handoff.

Handoff is restricted to Ethereum configurations that explicitly support it. BAL blocks and
EIP-8037 multidimensional gas are excluded. Custom configurations retain the existing prewarming
behavior unless they opt in and provide compatible transaction-result wrapping. No results are
shared across payloads or imported from txpool prewarming. Results are limited to a 128-transaction
lookahead and 8,192 distinct database reads per transaction.

Metrics under `sync.prewarm.handoff` count `reused`, `rejected`, and `missing` results. The comparison
tests cover state conflicts, beneficiary reads and fee rebasing, receipt and bundle equivalence,
reverts, halts, creation, self-destruction, block-gas admission, and bounded result lifetimes.

This is not a production rollout or a measured mainnet performance improvement. Before enabling it
by default, replay identical mainnet payloads with handoff enabled and disabled and compare gas,
receipts, state roots, throughput, tail latency, and memory consumption.
