//! Standalone functions for engine specific
pub mod payload;
pub use payload::{
    convert_standalonewithdraw_to_withdrawal, convert_withdrawal_to_standalonewithdraw,
    try_convert_from_execution_payload_v1_to_block,
    try_convert_from_sealed_block_to_execution_payload_v1, try_into_sealed_block,
};
