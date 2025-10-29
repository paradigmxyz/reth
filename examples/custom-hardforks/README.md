# Custom Hardforks Example

This example demonstrates how to define and integrate custom hardforks into a Reth-based chain. It shows:

- Defining a custom hardfork enum with variants (e.g., protocol upgrades).
- Implementing the `Hardfork` trait for behavior (e.g., feature gating).
- Configuring hardfork activation (e.g., via block number).
- Integrating hardforks into a custom chain spec and EVM configuration for feature gating.
- Running a minimal node to observe hardfork activation.

## Key Concepts

- **Hardfork Definition**: Use the `hardfork!` macro to create an enum of hardfork variants.
- **Trait Implementation**: Implement `Hardfork` to define names and logic.
- **Configuration**: A serializable struct for hardfork settings (e.g., activation blocks).
- **Integration**: Plug into `ChainSpec` and `EthEvmConfig` to gate EVM features (e.g., enable a precompile post-hardfork).
- **Activation Logic**: Hardforks activate based on `ForkCondition` (block, timestamp, or TTD).

## Running the Example

1. Build and run: `cargo run --release`
2. The node will start with a custom chain spec. Observe logs for hardfork activation (e.g., at block 10).
3. Test feature gating: Submit a transaction that uses the gated feature before/after activation.

## Extending

- Add new hardfork variants to the enum.
- Modify the trait to add custom logic (e.g., state transitions).
- Update the chain spec to include more hardforks.

Inspired by [alloy-rs/hardforks](https://github.com/alloy-rs/hardforks) and [berachain/bera-reth](https://github.com/berachain/bera-reth).