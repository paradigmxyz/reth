//! Types for representing call trace items.

use crate::tracing::utils::convert_memory;
use reth_primitives::{abi::decode_revert_reason, bytes::Bytes, Address, H256, U256};
use reth_rpc_types::trace::{
    geth::{CallFrame, CallLogFrame, GethDefaultTracingOptions, StructLog},
    parity::{
        Action, ActionType, CallAction, CallOutput, CallType, ChangedType, CreateAction,
        CreateOutput, Delta, SelfdestructAction, StateDiff, TraceOutput, TraceResult,
        TransactionTrace,
    },
};
use revm::interpreter::{
    opcode, CallContext, CallScheme, CreateScheme, InstructionResult, Memory, OpCode, Stack,
};
use serde::{Deserialize, Serialize};
use std::collections::{btree_map::Entry, VecDeque};

/// A unified representation of a call
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[allow(missing_docs)]
pub enum CallKind {
    #[default]
    Call,
    StaticCall,
    CallCode,
    DelegateCall,
    Create,
    Create2,
}

impl CallKind {
    /// Returns true if the call is a create
    pub fn is_any_create(&self) -> bool {
        matches!(self, CallKind::Create | CallKind::Create2)
    }
}

impl std::fmt::Display for CallKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallKind::Call => {
                write!(f, "CALL")
            }
            CallKind::StaticCall => {
                write!(f, "STATICCALL")
            }
            CallKind::CallCode => {
                write!(f, "CALLCODE")
            }
            CallKind::DelegateCall => {
                write!(f, "DELEGATECALL")
            }
            CallKind::Create => {
                write!(f, "CREATE")
            }
            CallKind::Create2 => {
                write!(f, "CREATE2")
            }
        }
    }
}

impl From<CallScheme> for CallKind {
    fn from(scheme: CallScheme) -> Self {
        match scheme {
            CallScheme::Call => CallKind::Call,
            CallScheme::StaticCall => CallKind::StaticCall,
            CallScheme::CallCode => CallKind::CallCode,
            CallScheme::DelegateCall => CallKind::DelegateCall,
        }
    }
}

impl From<CreateScheme> for CallKind {
    fn from(create: CreateScheme) -> Self {
        match create {
            CreateScheme::Create => CallKind::Create,
            CreateScheme::Create2 { .. } => CallKind::Create2,
        }
    }
}

impl From<CallKind> for ActionType {
    fn from(kind: CallKind) -> Self {
        match kind {
            CallKind::Call | CallKind::StaticCall | CallKind::DelegateCall | CallKind::CallCode => {
                ActionType::Call
            }
            CallKind::Create => ActionType::Create,
            CallKind::Create2 => ActionType::Create,
        }
    }
}

impl From<CallKind> for CallType {
    fn from(ty: CallKind) -> Self {
        match ty {
            CallKind::Call => CallType::Call,
            CallKind::StaticCall => CallType::StaticCall,
            CallKind::CallCode => CallType::CallCode,
            CallKind::DelegateCall => CallType::DelegateCall,
            CallKind::Create => CallType::None,
            CallKind::Create2 => CallType::None,
        }
    }
}

/// A trace of a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallTrace {
    /// The depth of the call
    pub(crate) depth: usize,
    /// Whether the call was successful
    pub(crate) success: bool,
    /// caller of this call
    pub(crate) caller: Address,
    /// The destination address of the call or the address from the created contract.
    ///
    /// In other words, this is the callee if the [CallKind::Call] or the address of the created
    /// contract if [CallKind::Create].
    pub(crate) address: Address,
    /// Whether this is a call to a precompile
    ///
    /// Note: This is an Option because not all tracers make use of this
    pub(crate) maybe_precompile: Option<bool>,
    /// Holds the target for the selfdestruct refund target if `status` is
    /// [InstructionResult::SelfDestruct]
    pub(crate) selfdestruct_refund_target: Option<Address>,
    /// The kind of call this is
    pub(crate) kind: CallKind,
    /// The value transferred in the call
    pub(crate) value: U256,
    /// The calldata for the call, or the init code for contract creations
    pub(crate) data: Bytes,
    /// The return data of the call if this was not a contract creation, otherwise it is the
    /// runtime bytecode of the created contract
    pub(crate) output: Bytes,
    /// The return data of the last call, if any
    pub(crate) last_call_return_value: Option<Bytes>,
    /// The gas cost of the call
    pub(crate) gas_used: u64,
    /// The status of the trace's call
    pub(crate) status: InstructionResult,
    /// call context of the runtime
    pub(crate) call_context: Option<CallContext>,
    /// Opcode-level execution steps
    pub(crate) steps: Vec<CallTraceStep>,
}

impl CallTrace {
    // Returns true if the status code is an error or revert, See [InstructionResult::Revert]
    pub(crate) fn is_error(&self) -> bool {
        self.status as u8 >= InstructionResult::Revert as u8
    }

    /// Returns the error message if it is an erroneous result.
    pub(crate) fn as_error(&self) -> Option<String> {
        self.is_error().then(|| format!("{:?}", self.status))
    }
}

impl Default for CallTrace {
    fn default() -> Self {
        Self {
            depth: Default::default(),
            success: Default::default(),
            caller: Default::default(),
            address: Default::default(),
            selfdestruct_refund_target: None,
            kind: Default::default(),
            value: Default::default(),
            data: Default::default(),
            maybe_precompile: None,
            output: Default::default(),
            last_call_return_value: None,
            gas_used: Default::default(),
            status: InstructionResult::Continue,
            call_context: Default::default(),
            steps: Default::default(),
        }
    }
}

/// A node in the arena
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallTraceNode {
    /// Parent node index in the arena
    pub(crate) parent: Option<usize>,
    /// Children node indexes in the arena
    pub(crate) children: Vec<usize>,
    /// This node's index in the arena
    pub(crate) idx: usize,
    /// The call trace
    pub(crate) trace: CallTrace,
    /// Logs
    pub(crate) logs: Vec<RawLog>,
    /// Ordering of child calls and logs
    pub(crate) ordering: Vec<LogCallOrder>,
}

impl CallTraceNode {
    /// Pushes all steps onto the stack in reverse order
    /// so that the first step is on top of the stack
    pub(crate) fn push_steps_on_stack<'a>(
        &'a self,
        stack: &mut VecDeque<CallTraceStepStackItem<'a>>,
    ) {
        stack.extend(self.call_step_stack().into_iter().rev());
    }

    /// Returns a list of all steps in this trace in the order they were executed
    ///
    /// If the step is a call, the id of the child trace is set.
    pub(crate) fn call_step_stack(&self) -> Vec<CallTraceStepStackItem<'_>> {
        let mut stack = Vec::with_capacity(self.trace.steps.len());
        let mut child_id = 0;
        for step in self.trace.steps.iter() {
            let mut item = CallTraceStepStackItem { trace_node: self, step, call_child_id: None };

            // If the opcode is a call, put the child trace on the stack
            match step.op.u8() {
                opcode::CREATE |
                opcode::CREATE2 |
                opcode::DELEGATECALL |
                opcode::CALL |
                opcode::STATICCALL |
                opcode::CALLCODE => {
                    let call_id = self.children[child_id];
                    item.call_child_id = Some(call_id);
                    child_id += 1;
                }
                _ => {}
            }
            stack.push(item);
        }
        stack
    }

    /// Returns true if this is a call to a precompile
    pub(crate) fn is_precompile(&self) -> bool {
        self.trace.maybe_precompile.unwrap_or(false)
    }

    /// Returns the kind of call the trace belongs to
    pub(crate) fn kind(&self) -> CallKind {
        self.trace.kind
    }

    /// Returns the status of the call
    pub(crate) fn status(&self) -> InstructionResult {
        self.trace.status
    }

    /// Updates the values of the state diff
    pub(crate) fn parity_update_state_diff(&self, diff: &mut StateDiff) {
        let addr = self.trace.address;
        let acc = diff.entry(addr).or_default();

        if self.kind().is_any_create() {
            let code = self.trace.output.clone();
            if acc.code == Delta::Unchanged {
                acc.code = Delta::Added(code.into())
            }
        }

        // TODO: track nonce and balance changes

        // iterate over all storage diffs
        for change in self.trace.steps.iter().filter_map(|s| s.storage_change) {
            let StorageChange { key, value, had_value } = change;
            let value = H256::from(value);
            match acc.storage.entry(key.into()) {
                Entry::Vacant(entry) => {
                    if let Some(had_value) = had_value {
                        entry.insert(Delta::Changed(ChangedType {
                            from: had_value.into(),
                            to: value,
                        }));
                    } else {
                        entry.insert(Delta::Added(value));
                    }
                }
                Entry::Occupied(mut entry) => {
                    let value = match entry.get() {
                        Delta::Unchanged => Delta::Added(value),
                        Delta::Added(added) => {
                            if added == &value {
                                Delta::Added(*added)
                            } else {
                                Delta::Changed(ChangedType { from: *added, to: value })
                            }
                        }
                        Delta::Removed(_) => Delta::Added(value),
                        Delta::Changed(c) => {
                            Delta::Changed(ChangedType { from: c.from, to: value })
                        }
                    };
                    entry.insert(value);
                }
            }
        }
    }

    /// Converts this node into a parity `TransactionTrace`
    pub(crate) fn parity_transaction_trace(&self, trace_address: Vec<usize>) -> TransactionTrace {
        let action = self.parity_action();
        let output = TraceResult::parity_success(self.parity_trace_output());
        TransactionTrace {
            action,
            result: Some(output),
            trace_address,
            subtraces: self.children.len(),
        }
    }

    /// Returns the `Output` for a parity trace
    pub(crate) fn parity_trace_output(&self) -> TraceOutput {
        match self.kind() {
            CallKind::Call | CallKind::StaticCall | CallKind::CallCode | CallKind::DelegateCall => {
                TraceOutput::Call(CallOutput {
                    gas_used: self.trace.gas_used.into(),
                    output: self.trace.output.clone().into(),
                })
            }
            CallKind::Create | CallKind::Create2 => TraceOutput::Create(CreateOutput {
                gas_used: self.trace.gas_used.into(),
                code: self.trace.output.clone().into(),
                address: self.trace.address,
            }),
        }
    }

    /// Returns the `Action` for a parity trace
    pub(crate) fn parity_action(&self) -> Action {
        if self.status() == InstructionResult::SelfDestruct {
            return Action::Selfdestruct(SelfdestructAction {
                address: self.trace.address,
                refund_address: self.trace.selfdestruct_refund_target.unwrap_or_default(),
                balance: self.trace.value,
            })
        }
        match self.kind() {
            CallKind::Call | CallKind::StaticCall | CallKind::CallCode | CallKind::DelegateCall => {
                Action::Call(CallAction {
                    from: self.trace.caller,
                    to: self.trace.address,
                    value: self.trace.value,
                    gas: self.trace.gas_used.into(),
                    input: self.trace.data.clone().into(),
                    call_type: self.kind().into(),
                })
            }
            CallKind::Create | CallKind::Create2 => Action::Create(CreateAction {
                from: self.trace.caller,
                value: self.trace.value,
                gas: self.trace.gas_used.into(),
                init: self.trace.data.clone().into(),
            }),
        }
    }

    /// Converts this call trace into an _empty_ geth [CallFrame]
    ///
    /// Caution: this does not include any of the child calls
    pub(crate) fn geth_empty_call_frame(&self, include_logs: bool) -> CallFrame {
        let mut call_frame = CallFrame {
            typ: self.trace.kind.to_string(),
            from: self.trace.caller,
            to: Some(self.trace.address),
            value: Some(self.trace.value),
            gas: U256::from(self.trace.gas_used),
            gas_used: U256::from(self.trace.gas_used),
            input: self.trace.data.clone().into(),
            output: Some(self.trace.output.clone().into()),
            error: None,
            revert_reason: None,
            calls: None,
            logs: None,
        };

        // we need to populate error and revert reason
        if !self.trace.success {
            call_frame.revert_reason = decode_revert_reason(self.trace.output.clone());
            call_frame.error = self.trace.as_error();
        }

        if include_logs {
            call_frame.logs = Some(
                self.logs
                    .iter()
                    .map(|log| CallLogFrame {
                        address: Some(self.trace.address),
                        topics: Some(log.topics.clone()),
                        data: Some(log.data.clone().into()),
                    })
                    .collect(),
            );
        }

        call_frame
    }
}

pub(crate) struct CallTraceStepStackItem<'a> {
    /// The trace node that contains this step
    pub(crate) trace_node: &'a CallTraceNode,
    /// The step that this stack item represents
    pub(crate) step: &'a CallTraceStep,
    /// The index of the child call in the CallArena if this step's opcode is a call
    pub(crate) call_child_id: Option<usize>,
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

/// Ethereum log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawLog {
    /// Indexed event params are represented as log topics.
    pub(crate) topics: Vec<H256>,
    /// Others are just plain data.
    pub(crate) data: Bytes,
}

/// Represents a tracked call step during execution
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallTraceStep {
    // Fields filled in `step`
    /// Call depth
    pub(crate) depth: u64,
    /// Program counter before step execution
    pub(crate) pc: usize,
    /// Opcode to be executed
    pub(crate) op: OpCode,
    /// Current contract address
    pub(crate) contract: Address,
    /// Stack before step execution
    pub(crate) stack: Stack,
    /// All allocated memory in a step
    ///
    /// This will be empty if memory capture is disabled
    pub(crate) memory: Memory,
    /// Size of memory
    pub(crate) memory_size: usize,
    /// Remaining gas before step execution
    pub(crate) gas_remaining: u64,
    /// Gas refund counter before step execution
    pub(crate) gas_refund_counter: u64,
    // Fields filled in `step_end`
    /// Gas cost of step execution
    pub(crate) gas_cost: u64,
    /// Change of the contract state after step execution (effect of the SLOAD/SSTORE instructions)
    pub(crate) storage_change: Option<StorageChange>,
    /// Final status of the call
    pub(crate) status: InstructionResult,
}

// === impl CallTraceStep ===

impl CallTraceStep {
    /// Converts this step into a geth [StructLog]
    ///
    /// This sets memory and stack capture based on the `opts` parameter.
    pub(crate) fn convert_to_geth_struct_log(&self, opts: &GethDefaultTracingOptions) -> StructLog {
        let mut log = StructLog {
            depth: self.depth,
            error: self.as_error(),
            gas: self.gas_remaining,
            gas_cost: self.gas_cost,
            op: self.op.to_string(),
            pc: self.pc as u64,
            refund_counter: (self.gas_refund_counter > 0).then_some(self.gas_refund_counter),
            // Filled, if not disabled manually
            stack: None,
            // Filled in `CallTraceArena::geth_trace` as a result of compounding all slot changes
            return_data: None,
            // Filled via trace object
            storage: None,
            // Only enabled if `opts.enable_memory` is true
            memory: None,
            // This is None in the rpc response
            memory_size: None,
        };

        if opts.is_stack_enabled() {
            log.stack = Some(self.stack.data().clone());
        }

        if opts.is_memory_enabled() {
            log.memory = Some(convert_memory(self.memory.data()));
        }

        log
    }

    // Returns true if the status code is an error or revert, See [InstructionResult::Revert]
    pub(crate) fn is_error(&self) -> bool {
        self.status as u8 >= InstructionResult::Revert as u8
    }

    /// Returns the error message if it is an erroneous result.
    pub(crate) fn as_error(&self) -> Option<String> {
        self.is_error().then(|| format!("{:?}", self.status))
    }
}

/// Represents a storage change during execution
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StorageChange {
    pub(crate) key: U256,
    pub(crate) value: U256,
    pub(crate) had_value: Option<U256>,
}
