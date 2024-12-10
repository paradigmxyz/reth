//! Scroll consensus implementation.

#![cfg(feature = "scroll")]

mod curie;
pub use curie::{
    apply_curie_hard_fork, BLOB_SCALAR_SLOT, COMMIT_SCALAR_SLOT,
    CURIE_L1_GAS_PRICE_ORACLE_BYTECODE, CURIE_L1_GAS_PRICE_ORACLE_STORAGE, IS_CURIE_SLOT,
    L1_BASE_FEE_SLOT, L1_BLOB_BASE_FEE_SLOT, L1_GAS_PRICE_ORACLE_ADDRESS, OVER_HEAD_SLOT,
    SCALAR_SLOT,
};
