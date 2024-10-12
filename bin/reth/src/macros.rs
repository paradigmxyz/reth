//! Helper macros

/// Creates the block executor type based on the configured feature.
///
/// Note(mattsse): This is incredibly horrible and will be replaced
#[cfg(not(feature = "optimism"))]
macro_rules! block_executor {
    ($chain_spec:expr) => {
        reth_node_ethereum::EthExecutorProvider::ethereum($chain_spec)
    };
}

#[cfg(feature = "optimism")]
macro_rules! block_executor {
    ($chain_spec:expr) => {
        reth_node_optimism::OpExecutorProvider::optimism($chain_spec)
    };
}

pub(crate) use block_executor;
