#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod builder;
mod constants;
mod entry;
mod store;
mod table;

pub use builder::{
    build_entries_for_block, build_entries_from_block_data, encode_get_calldata,
    encode_set_calldata,
};
pub use constants::*;
pub use entry::IndexEntry;
pub use store::IndexTableStore;
pub use table::IndexTable;
