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
Transactions with later sender nonces or funding dependencies can fail strict parent-state
speculation. They are retried with permissive cache-only warming, and those results are never
published for reuse. Transactions outside the bounded lookahead also use cache-only warming.

Workers also record ordered `SSTORE` operations with call/create frame checkpoints. Failed frames
discard their writes, but retain their read dependencies. Before publication, the recorder checks
that replay covers the worker's entire committed storage delta, then resets stored slot values to
their originals. Canonical execution applies only the recorded committed writes after validating
all preconditions; it does not import the worker's final storage snapshot. Creation/destruction and
storage clearing retain REVM's account status metadata and ordinary executor commit semantics.

Balance effects replay as additions/subtractions onto the current balance when execution did not
observe the balance. `BALANCE`, `SELFBALANCE`, `SELFDESTRUCT`, and internal value-transfer checks
remain exact dependencies, including reads in reverted frames or failed value transfers. Caller
funding conservatively requires at least its parent-state balance. Existence, nonce, code hash, and
empty-account classification must still match; a beneficiary loaded only after execution is exempt
from the empty-account check because its fee-only access cannot influence call gas. Arithmetic
overflow/underflow rejects the entire handoff before canonical commit.

This follows Nethermind's separation of read preconditions and replayable effects, not its complete
implementation. Account metadata checks and the caller minimum are deliberately more conservative;
sender-chain warming and Nethermind's fine-grained per-field/minimum-balance tracking are not ported.

Handoff is restricted to Ethereum configurations that explicitly support it. BAL blocks and
EIP-8037 multidimensional gas are excluded. Custom configurations retain the existing prewarming
behavior unless they opt in and provide compatible transaction-result wrapping. No results are
shared across payloads or imported from txpool prewarming. Results are limited to a 128-transaction
lookahead and 8,192 distinct database reads, balance observations, or storage actions per transaction.

Metrics under `sync.prewarm.handoff` count `reused`, `rejected`, and `missing` results. The comparison
tests cover state conflicts, beneficiary reads and fee rebasing, receipt and bundle equivalence,
reverts, halts, creation, self-destruction, block-gas admission, and bounded result lifetimes.

This is not a production rollout or a measured mainnet performance improvement. Before enabling it
by default, replay identical mainnet payloads with handoff enabled and disabled and compare gas,
receipts, state roots, throughput, tail latency, and memory consumption.
