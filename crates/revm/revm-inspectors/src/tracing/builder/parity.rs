use crate::tracing::{types::CallTraceNode, TracingInspectorConfig};
use reth_rpc_types::{trace::parity::*, TransactionInfo};

/// A type for creating parity style traces
#[derive(Clone, Debug)]
pub struct ParityTraceBuilder {
    /// Recorded trace nodes
    nodes: Vec<CallTraceNode>,
    /// How the traces were recorded
    _config: TracingInspectorConfig,
}

impl ParityTraceBuilder {
    /// Returns a new instance of the builder
    pub(crate) fn new(nodes: Vec<CallTraceNode>, _config: TracingInspectorConfig) -> Self {
        Self { nodes, _config }
    }

    /// Returns the trace addresses of all transactions in the set
    fn trace_addresses(&self) -> Vec<Vec<usize>> {
        let mut all_addresses = Vec::with_capacity(self.nodes.len());
        for idx in 0..self.nodes.len() {
            all_addresses.push(self.trace_address(idx));
        }
        all_addresses
    }

    /// Returns the `traceAddress` of the node in the arena
    ///
    /// The `traceAddress` field of all returned traces, gives the exact location in the call trace
    /// [index in root, index in first CALL, index in second CALL, …].
    ///
    /// # Panics
    ///
    /// if the `idx` does not belong to a node
    fn trace_address(&self, idx: usize) -> Vec<usize> {
        if idx == 0 {
            // root call has empty traceAddress
            return vec![]
        }
        let mut graph = vec![];
        let mut node = &self.nodes[idx];
        while let Some(parent) = node.parent {
            // the index of the child call in the arena
            let child_idx = node.idx;
            node = &self.nodes[parent];
            // find the index of the child call in the parent node
            let call_idx = node
                .children
                .iter()
                .position(|child| *child == child_idx)
                .expect("child exists in parent");
            graph.push(call_idx);
        }
        graph.reverse();
        graph
    }

    /// Returns an iterator over all recorded traces  for `trace_transaction`
    pub fn into_localized_transaction_traces_iter(
        self,
        info: TransactionInfo,
    ) -> impl Iterator<Item = LocalizedTransactionTrace> {
        self.into_transaction_traces_iter().map(move |trace| {
            let TransactionInfo { hash, index, block_hash, block_number } = info;
            LocalizedTransactionTrace {
                trace,
                transaction_position: index,
                transaction_hash: hash,
                block_number,
                block_hash,
            }
        })
    }

    /// Returns an iterator over all recorded traces  for `trace_transaction`
    pub fn into_localized_transaction_traces(
        self,
        info: TransactionInfo,
    ) -> Vec<LocalizedTransactionTrace> {
        self.into_localized_transaction_traces_iter(info).collect()
    }

    /// Returns an iterator over all recorded traces  for `trace_transaction`
    pub fn into_transaction_traces_iter(self) -> impl Iterator<Item = TransactionTrace> {
        let trace_addresses = self.trace_addresses();

        self.nodes.into_iter().zip(trace_addresses).map(|(node, trace_address)| {
            let action = node.parity_action();
            let output = TraceResult::parity_success(node.parity_trace_output());
            TransactionTrace {
                action,
                result: Some(output),
                trace_address,
                subtraces: node.children.len(),
            }
        })
    }

    /// Returns the raw traces of the transaction
    pub fn into_transaction_traces(self) -> Vec<TransactionTrace> {
        self.into_transaction_traces_iter().collect()
    }
}
