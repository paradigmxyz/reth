//! Header fixtures shared by the crate's tests.

use crate::SnapPivotPolicy;
use alloy_consensus::Header;
use alloy_primitives::B256;
use reth_provider::{
    test_utils::create_test_provider_factory, DatabaseProviderFactory, StaticFileProviderFactory,
    StaticFileWriter,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::HeaderProvider;

/// Small bounds keep header fixtures short without changing the policy's decisions.
pub(crate) fn policy() -> SnapPivotPolicy {
    SnapPivotPolicy::default().with_head_distance(1).with_advance_after(4).with_history(8)
}

/// A header with a state root distinctive to its number, and a commitment when one is given.
pub(crate) fn header(
    number: u64,
    parent_hash: B256,
    block_access_list_hash: Option<B256>,
) -> Header {
    Header {
        number,
        parent_hash,
        state_root: B256::repeat_byte(number as u8),
        block_access_list_hash,
        ..Default::default()
    }
}

/// Blocks `0..=3`, carrying a block access list commitment from `bal_from` onwards.
pub(crate) fn chain(bal_from: Option<u64>) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut parent = B256::ZERO;
    for number in 0..=3 {
        let bal =
            bal_from.filter(|from| number >= *from).map(|_| B256::with_last_byte(number as u8));
        let header = header(number, parent, bal);
        parent = header.hash_slow();
        headers.push(header);
    }
    headers
}

pub(crate) fn provider_with(
    headers: impl IntoIterator<Item = Header>,
) -> impl HeaderProvider<Header = Header> {
    let factory = create_test_provider_factory();
    let static_files = factory.static_file_provider();
    let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
    for header in headers {
        let hash = header.hash_slow();
        writer.append_header(&header, &hash).unwrap();
    }
    writer.commit().unwrap();
    drop(writer);
    drop(static_files);
    factory.database_provider_ro().unwrap()
}
