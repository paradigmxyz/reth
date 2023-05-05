//! Types for representing call trace items.

use crate::tracing::utils::convert_memory;
use reth_primitives::{bytes::Bytes, Address, H256, U256};
use reth_rpc_types::trace::{
    geth::StructLog,
    parity::{
        Action, ActionType, CallAction, CallOutput, CallType, ChangedType, CreateAction,
        CreateOutput, Delta, SelfdestructAction, StateDiff, TraceOutput, TraceResult,
        TransactionTrace,
    },
};
use revm::interpreter::{
    CallContext, CallScheme, CreateScheme, InstructionResult, Memory, OpCode, Stack,
};
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;

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
    /// Size of memory
    pub memory_size: usize,
    /// Remaining gas before step execution
    pub gas: u64,
    /// Gas refund counter before step execution
    pub gas_refund_counter: u64,
    // Fields filled in `step_end`
    /// Gas cost of step execution
    pub gas_cost: u64,
    /// Change of the contract state after step execution (effect of the SLOAD/SSTORE instructions)
    pub storage_change: Option<StorageChange>,
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
        self.is_error().then(|| format!("{:?}", self.status))
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
            refund_counter: (step.gas_refund_counter > 0).then_some(step.gas_refund_counter),
            stack: Some(step.stack.data().clone()),
            // Filled in `CallTraceArena::geth_trace` as a result of compounding all slot changes
            return_data: None,
            storage: None,
            memory_size: step.memory_size as u64,
        }
    }
}

/// Represents a storage change during execution
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageChange {
    pub key: U256,
    pub value: U256,
    pub had_value: Option<U256>,
}
