### Describe the issue

The `on_forkchoice_updated()` function in the engine tree module currently handles multiple responsibilities in a single 147-line method, making it difficult to understand and maintain.

Current implementation:
https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1011-L1158

The function mixes several distinct concerns:
- Pre-validation of forkchoice state
- Checking if head is already canonical
- Processing payload attributes
- Handling reorgs for OpStack
- Updating canonical chain
- Downloading missing blocks
- Safe/finalized block validation

This creates deeply nested conditional logic with multiple early returns scattered throughout, making the control flow hard to follow.

### Current implementation

The current implementation spans 147 lines with multiple nested conditions and mixed responsibilities:

https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1011-L1158

Key sections within the function:
- Pre-validation and metrics: https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1017-L1026
- Head already canonical check: https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1050-L1074
- Canonical chain ancestor check: https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1077-L1113
- Chain update application: https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1116-L1132
- Missing block handling: https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L1134-L1158

### Proposed solution

Refactor the function to separate concerns into focused helper methods:

```rust
fn on_forkchoice_updated(
    &mut self,
    state: ForkchoiceState,
    attrs: Option<T::PayloadAttributes>,
    version: EngineApiMessageVersion,
) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
    self.record_forkchoice_metrics(&attrs);

    if let Some(early_result) = self.validate_forkchoice_state(state)? {
        return Ok(TreeOutcome::new(early_result));
    }

    if let Some(result) = self.handle_canonical_head(state, attrs, version)? {
        return Ok(result);
    }

    if let Some(result) = self.apply_chain_update(state, attrs, version)? {
        return Ok(result);
    }

    self.handle_missing_block(state)
}

// Helper methods with single responsibilities:
fn validate_forkchoice_state(&mut self, state: ForkchoiceState) -> ProviderResult<Option<OnForkChoiceUpdated>>;
fn handle_canonical_head(&mut self, state: ForkchoiceState, attrs: Option<T::PayloadAttributes>, version: EngineApiMessageVersion) -> ProviderResult<Option<TreeOutcome<OnForkChoiceUpdated>>>;
fn apply_chain_update(&mut self, state: ForkchoiceState, attrs: Option<T::PayloadAttributes>, version: EngineApiMessageVersion) -> ProviderResult<Option<TreeOutcome<OnForkChoiceUpdated>>>;
fn handle_missing_block(&mut self, state: ForkchoiceState) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>>;
fn process_payload_attributes_if_present(&mut self, attrs: Option<T::PayloadAttributes>, tip: &SealedHeader, state: ForkchoiceState, version: EngineApiMessageVersion) -> OnForkChoiceUpdated;
```

This refactoring will improve code readability, maintainability, and testability by separating each logical concern into its own focused method.

### Additional context

Related to parent issue #18447