//! Standalone functions for engine specific rpc type conversions
pub mod payload;
pub use payload::{
    convert_standalonewithdraw_to_withdrawal, convert_withdrawal_to_standalonewithdraw,
    try_block_to_payload_v1, try_into_sealed_block, try_payload_v1_to_block,
};
