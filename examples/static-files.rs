use std::time::Duration;

use reth_db::test_utils::create_test_static_files_dir;
use reth_primitives::StaticFileSegment;
use reth_provider::providers::{StaticFileProvider, StaticFileWriter};

fn main() -> eyre::Result<()> {
    for _ in 0..100 {
        let static_file_provider = StaticFileProvider::new(create_test_static_files_dir())?;
        let _writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;
    }

    std::thread::sleep(Duration::from_secs(10 * 60));

    Ok(())
}
