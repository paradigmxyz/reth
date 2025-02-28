//! Chain specification for the Base Sepolia testnet network.

use alloc::sync::Arc;

use crate::{superchain_registry::read_superchain_genesis, LazyLock, OpChainSpec};

/// The Base Sepolia spec
pub static BASE_SEPOLIA: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    let mut op_chain_spec = OpChainSpec::from(
        read_superchain_genesis("base", "sepolia").expect("Can't read Base Sepolia genesis"),
    );
    op_chain_spec.inner.prune_delete_limit = 10000;
    op_chain_spec.into()
});
