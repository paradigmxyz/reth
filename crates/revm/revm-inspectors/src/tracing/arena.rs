use crate::tracing::{
    call::{CallTrace},
    node::CallTraceNode,
};
use reth_primitives::{hex, Address, U256};
use reth_rpc_types::trace::{DefaultFrame, GethDebugTracingOptions, StructLog};
use revm::interpreter::{InstructionResult, Memory, OpCode, Stack};
use revm::interpreter::instruction_result::SuccessOrHalt;

/// An arena of [CallTraceNode]s
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CallTraceArena {
    /// The arena of nodes
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

    // // Recursively fill in the geth trace by going through the traces
    // fn add_to_geth_trace(
    //     &self,
    //     storage: &mut HashMap<Address, BTreeMap<H256, H256>>,
    //     trace_node: &CallTraceNode,
    //     struct_logs: &mut Vec<StructLog>,
    //     opts: &GethDebugTracingOptions,
    // ) {
    //     let mut child_id = 0;
    //     // Iterate over the steps inside the given trace
    //     for step in trace_node.trace.steps.iter() {
    //         let mut log: StructLog = step.into();
    //
    //         // Fill in memory and storage depending on the options
    //         if !opts.disable_storage.unwrap_or_default() {
    //             let contract_storage = storage.entry(step.contract).or_default();
    //             if let Some((key, value)) = step.state_diff {
    //                 contract_storage.insert(H256::from_uint(&key), H256::from_uint(&value));
    //                 log.storage = Some(contract_storage.clone());
    //             }
    //         }
    //         if opts.disable_stack.unwrap_or_default() {
    //             log.stack = None;
    //         }
    //         if !opts.enable_memory.unwrap_or_default() {
    //             log.memory = None;
    //         }
    //
    //         // Add step to geth trace
    //         struct_logs.push(log);
    //
    //         // Check if the step was a call
    //         match step.op {
    //             Instruction::OpCode(opc) => {
    //                 match opc {
    //                     // If yes, descend into a child trace
    //                     opcode::CREATE |
    //                     opcode::CREATE2 |
    //                     opcode::DELEGATECALL |
    //                     opcode::CALL |
    //                     opcode::STATICCALL |
    //                     opcode::CALLCODE => {
    //                         self.add_to_geth_trace(
    //                             storage,
    //                             &self.arena[trace_node.children[child_id]],
    //                             struct_logs,
    //                             opts,
    //                         );
    //                         child_id += 1;
    //                     }
    //                     _ => {}
    //                 }
    //             }
    //             Instruction::Cheatcode(_) => {}
    //         }
    //     }
    // }

    /// Generate a geth-style trace e.g. for debug_traceTransaction
    pub(crate) fn geth_trace(
        &self,
        _receipt_gas_used: U256,
        _opts: GethDebugTracingOptions,
    ) -> DefaultFrame {
        if self.arena.is_empty() {
            return Default::default()
        }
        unimplemented!()
        // let mut storage = HashMap::new();
        // // Fetch top-level trace
        // let main_trace_node = &self.arena[0];
        // let main_trace = &main_trace_node.trace;
        // // Start geth trace
        // let mut acc = DefaultFrame {
        //     // If the top-level trace succeeded, then it was a success
        //     failed: !main_trace.success,
        //     gas: receipt_gas_used,
        //     return_value: main_trace.output.to_bytes(),
        //     ..Default::default()
        // };
        //
        // self.add_to_geth_trace(&mut storage, main_trace_node, &mut acc.struct_logs, &opts);
        //
        // acc
    }
}

/// Ordering enum for calls and logs
///
/// i.e. if Call 0 occurs before Log 0, it will be pushed into the `CallTraceNode`'s ordering before
/// the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogCallOrder {
    Log(usize),
    Call(usize),
}

/// Represents a tracked call step during execution
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallTraceStep {
    // Fields filled in `step`
    /// Call depth
    pub depth: u64,
    /// Program counter before step execution
    pub pc: usize,
    /// Opcode to be executed
    pub op: OpCode,
    /// Current contract address
    pub contract: Address,
    /// Stack before step execution
    pub stack: Stack,
    /// Memory before step execution
    pub memory: Memory,
    /// Remaining gas before step execution
    pub gas: u64,
    /// Gas refund counter before step execution
    pub gas_refund_counter: u64,
    // Fields filled in `step_end`
    /// Gas cost of step execution
    pub gas_cost: u64,
    /// Change of the contract state after step execution (effect of the SLOAD/SSTORE instructions)
    pub state_diff: Option<(U256, U256)>,
    /// Final status of the call
    pub status: InstructionResult,
}

// === impl CallTraceStep ===

impl CallTraceStep {
    // Returns true if the status code is an error or revert, See [InstructionResult::Revert]
    pub fn is_error(&self) -> bool {
        self.status as u8 >= InstructionResult::Revert as u8
    }

    /// Returns the error message if it is an erroneous result.
    pub fn as_error(&self) -> Option<String> {
        if self.is_error() {
            Some(format!("{:?}", self.status))
        } else {
            None
        }
    }
}

impl From<&CallTraceStep> for StructLog {
    fn from(step: &CallTraceStep) -> Self {
        StructLog {
            depth: step.depth,
            error: step.as_error(),
            gas: step.gas,
            gas_cost: step.gas_cost,
            memory: Some(convert_memory(step.memory.data())),
            op: step.op.to_string(),
            pc: step.pc as u64,
            refund_counter: if step.gas_refund_counter > 0 {
                Some(step.gas_refund_counter)
            } else {
                None
            },
            stack: Some(step.stack.data().iter().copied().map(Into::into).collect()),
            // Filled in `CallTraceArena::geth_trace` as a result of compounding all slot changes
            storage: None,
        }
    }
}

// === impl CallTrace ===

/// Specifies the kind of trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TraceKind {
    Deployment,
    Setup,
    Execution,
}

/// creates the memory data in 32byte chunks
/// see <https://github.com/ethereum/go-ethereum/blob/366d2169fbc0e0f803b68c042b77b6b480836dbc/eth/tracers/logger/logger.go#L450-L452>
fn convert_memory(data: &[u8]) -> Vec<String> {
    let mut memory = Vec::with_capacity((data.len() + 31) / 32);
    for idx in (0..data.len()).step_by(32) {
        let len = std::cmp::min(idx + 32, data.len());
        memory.push(hex::encode(&data[idx..len]));
    }
    memory
}
