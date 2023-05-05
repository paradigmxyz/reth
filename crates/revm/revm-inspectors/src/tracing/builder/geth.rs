//! Geth trace builder

use crate::tracing::{types::CallTraceNode, TracingInspectorConfig};
use reth_primitives::{Address, JsonU256, H256, U256};
use reth_rpc_types::trace::geth::*;
use revm::interpreter::opcode;
use std::collections::{BTreeMap, HashMap};

/// A type for creating geth style traces
#[derive(Clone, Debug)]
pub struct GethTraceBuilder {
    /// Recorded trace nodes.
    nodes: Vec<CallTraceNode>,
    /// How the traces were recorded
    _config: TracingInspectorConfig,
}

impl GethTraceBuilder {
    /// Returns a new instance of the builder
    pub(crate) fn new(nodes: Vec<CallTraceNode>, _config: TracingInspectorConfig) -> Self {
        Self { nodes, _config }
    }

    /// Recursively fill in the geth trace by going through the traces
    ///
    /// TODO rewrite this iteratively
    fn add_to_geth_trace(
        &self,
        storage: &mut HashMap<Address, BTreeMap<H256, H256>>,
        trace_node: &CallTraceNode,
        struct_logs: &mut Vec<StructLog>,
        opts: &GethDefaultTracingOptions,
    ) {
        let mut child_id = 0;
        // Iterate over the steps inside the given trace
        for step in trace_node.trace.steps.iter() {
            let mut log: StructLog = step.into();

            // Fill in memory and storage depending on the options
            if !opts.disable_storage.unwrap_or_default() {
                let contract_storage = storage.entry(step.contract).or_default();
                if let Some(change) = step.storage_change {
                    contract_storage.insert(change.key.into(), change.value.into());
                    log.storage = Some(contract_storage.clone());
                }
            }
            if opts.disable_stack.unwrap_or_default() {
                log.stack = None;
            }

            if !opts.enable_memory.unwrap_or_default() {
                log.memory = None;
            }

            if opts.enable_return_data.unwrap_or_default() {
                log.return_data = trace_node.trace.last_call_return_value.clone().map(Into::into);
            }

            // Add step to geth trace
            struct_logs.push(log);

            // If the opcode is a call, the descend into child trace
            match step.op.u8() {
                opcode::CREATE |
                opcode::CREATE2 |
                opcode::DELEGATECALL |
                opcode::CALL |
                opcode::STATICCALL |
                opcode::CALLCODE => {
                    self.add_to_geth_trace(
                        storage,
                        &self.nodes[trace_node.children[child_id]],
                        struct_logs,
                        opts,
                    );
                    child_id += 1;
                }
                _ => {}
            }
        }
    }

    /// Generate a geth-style trace e.g. for `debug_traceTransaction`
    pub fn geth_traces(
        &self,
        // TODO(mattsse): This should be the total gas used, or gas used by last CallTrace?
        receipt_gas_used: U256,
        opts: GethDefaultTracingOptions,
    ) -> DefaultFrame {
        if self.nodes.is_empty() {
            return Default::default()
        }
        // Fetch top-level trace
        let main_trace_node = &self.nodes[0];
        let main_trace = &main_trace_node.trace;

        let mut struct_logs = Vec::new();
        let mut storage = HashMap::new();
        self.add_to_geth_trace(&mut storage, main_trace_node, &mut struct_logs, &opts);

        DefaultFrame {
            // If the top-level trace succeeded, then it was a success
            failed: !main_trace.success,
            gas: JsonU256(receipt_gas_used),
            return_value: main_trace.output.clone().into(),
            struct_logs,
        }
    }
}
