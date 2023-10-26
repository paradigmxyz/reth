
/// Convert from [reth_primitives::NodeRecord] to [reth_rpc_types::NodeRecord]
pub fn from_primitive_node_record(node_record: reth_primitives::NodeRecord) -> reth_rpc_types::NodeRecord {
    reth_rpc_types::NodeRecord { address: node_record.address, tcp_port: node_record.tcp_port, udp_port: node_record.udp_port, id: node_record.id }
}