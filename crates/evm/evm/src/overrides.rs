//! RPC override helpers for EVM environments.

pub use alloy_evm::overrides::{
    apply_block_overrides, apply_state_overrides, OverrideBlockHashes, StateOverrideError,
};
