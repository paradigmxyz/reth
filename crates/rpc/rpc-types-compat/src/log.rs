/// Creates a new rpc Log from a primitive log type from DB
pub fn from_primitive(log: reth_primitives::Log) -> reth_rpc_types::Log {
    reth_rpc_types::Log {
        address: log.address,
        topics: log.topics,
        data: log.data,
        block_hash: None,
        block_number: None,
        transaction_hash: None,
        transaction_index: None,
        log_index: None,
        removed: false,
    }
}
