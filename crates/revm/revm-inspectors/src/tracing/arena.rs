use crate::tracing::types::{CallTrace, CallTraceNode, LogCallOrder};
use reth_primitives::{Address, JsonU256, H256, U256};
use reth_rpc_types::trace::{
    geth::{DefaultFrame, GethDebugTracingOptions, StructLog},
    parity::{ActionType, TransactionTrace},
};
use revm::interpreter::{opcode, InstructionResult};
use std::collections::{BTreeMap, HashMap};

/// An arena of recorded traces.
///
/// This type will be populated via the [TracingInspector](crate::tracing::TracingInspector).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallTraceArena {
    /// The arena of recorded trace nodes
    pub(crate) arena: Vec<CallTraceNode>,
}

impl CallTraceArena {
    /// Pushes a new trace into the arena, returning the trace ID
    pub(crate) fn push_trace(&mut self, entry: usize, new_trace: CallTrace) -> usize {
        match new_trace.depth {
            // The entry node, just update it
            0 => {
                self.arena[0].trace = new_trace;
                0
            }
            // We found the parent node, add the new trace as a child
            _ if self.arena[entry].trace.depth == new_trace.depth - 1 => {
                let id = self.arena.len();

                let trace_location = self.arena[entry].children.len();
                self.arena[entry].ordering.push(LogCallOrder::Call(trace_location));
                let node = CallTraceNode {
                    parent: Some(entry),
                    trace: new_trace,
                    idx: id,
                    ..Default::default()
                };
                self.arena.push(node);
                self.arena[entry].children.push(id);

                id
            }
            // We haven't found the parent node, go deeper
            _ => self.push_trace(
                *self.arena[entry].children.last().expect("Disconnected trace"),
                new_trace,
            ),
        }
    }

    /// Returns the traces of the transaction for `trace_transaction`
    pub fn parity_traces(&self) -> Vec<TransactionTrace> {
        let traces = Vec::with_capacity(self.arena.len());
        for (_idx, node) in self.arena.iter().cloned().enumerate() {
            let _action = node.parity_action();
            let _result = node.parity_result();

            let _action_type = if node.status() == InstructionResult::SelfDestruct {
                ActionType::Selfdestruct
            } else {
                node.kind().into()
            };

            todo!()

            // let trace = TransactionTrace {
            //     action,
            //     result: Some(result),
            //     trace_address: self.info.trace_address(idx),
            //     subtraces: node.children.len(),
            // };
            // traces.push(trace)
        }

        traces
    }

    /// Recursively fill in the geth trace by going through the traces
    ///
    /// TODO rewrite this iteratively
    fn add_to_geth_trace(
        &self,
        storage: &mut HashMap<Address, BTreeMap<H256, H256>>,
        trace_node: &CallTraceNode,
        struct_logs: &mut Vec<StructLog>,
        opts: &GethDebugTracingOptions,
    ) {
        let mut child_id = 0;
        // Iterate over the steps inside the given trace
        for step in trace_node.trace.steps.iter() {
            let mut log: StructLog = step.into();

            // Fill in memory and storage depending on the options
            if !opts.disable_storage.unwrap_or_default() {
                let contract_storage = storage.entry(step.contract).or_default();
                if let Some((key, value)) = step.state_diff {
                    contract_storage.insert(key.into(), value.into());
                    log.storage = Some(contract_storage.clone());
                }
            }
            if opts.disable_stack.unwrap_or_default() {
                log.stack = None;
            }
            if !opts.enable_memory.unwrap_or_default() {
                log.memory = None;
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
                        &self.arena[trace_node.children[child_id]],
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
        opts: GethDebugTracingOptions,
    ) -> DefaultFrame {
        if self.arena.is_empty() {
            return Default::default()
        }
        // Fetch top-level trace
        let main_trace_node = &self.arena[0];
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
