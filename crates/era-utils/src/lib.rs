//! Moves block history between ERA files ([`reth_era`]) and node storage ([`reth_storage_api`]).
//!
//! Each ERA format plugs into a shared pipeline through a per-format seam ([`EraBlockReader`]).

mod history;

mod export;

pub use export::{export, EraBlockWriter, ExportBlock, ExportConfig};

pub use history::{
    build_index, calculate_td_by_number, decode, import, open, process, process_iter,
    save_stage_checkpoints, Era, Era1, EraBlockReader, Ere,
};
