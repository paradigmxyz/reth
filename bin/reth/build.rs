#![allow(missing_docs)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    reth_node_version_build::emit_version_metadata()
}
