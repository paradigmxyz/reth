//! Chain specification for the Base Mainnet network.

use alloc::sync::Arc;

use crate::{superchain_registry::read_superchain_genesis, LazyLock, OpChainSpec};

/// The Base mainnet spec
pub static BASE_MAINNET: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    OpChainSpec::from(read_superchain_genesis("base", "mainnet").expect("Can't read Base genesis"))
        .into()
});
