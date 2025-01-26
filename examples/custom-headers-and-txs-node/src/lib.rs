//! This example shows how implement custom Headers

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod block;
mod header;
mod primitives;
pub use header::CustomHeader;
use reth::primitives::Header;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let header = Header { number: 10, ..Default::default() };
    let data = "extra extra data".to_string();

    let custom_header = CustomHeader::new(header, data);

    println!("{:?}", custom_header);

    Ok(())
}
