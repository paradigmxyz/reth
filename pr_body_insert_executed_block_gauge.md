Fixes stale `consensus.engine.beacon.executed_blocks` reporting on the `InsertExecutedBlock` path.

`EngineApiRequest::InsertExecutedBlock` inserts into `tree_state`, but previously did not refresh the `executed_blocks` gauge. This change updates the gauge immediately after insertion so observability stays aligned with in-memory state.

Adds a focused tree test that exercises `InsertExecutedBlock` handling and verifies insertion plus emitted canonical-add event behavior.

## How to test

- `cargo test -p reth-engine-tree test_insert_executed_block_updates_tree_state_and_emits_event`
- `cargo clippy -p reth-engine-tree --tests --no-deps -- -D warnings`
- `cargo +nightly fmt --check`
