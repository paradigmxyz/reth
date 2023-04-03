use crate::tracing::{
    types::{CallKind, LogCallOrder, RawLog},
    utils::{gas_used, get_create_address},
};
pub use arena::CallTraceArena;
use reth_primitives::{bytes::Bytes, Address, H256, U256};
use revm::{
    inspectors::GasInspector,
    interpreter::{
        opcode, return_ok, CallInputs, CallScheme, CreateInputs, Gas, InstructionResult,
        Interpreter, OpCode,
    },
    Database, EVMData, Inspector, JournalEntry,
};
use types::{CallTrace, CallTraceStep};

mod arena;
mod builder;
mod config;
mod fourbyte;
mod types;
mod utils;
pub use builder::{geth::GethTraceBuilder, parity::ParityTraceBuilder};
pub use config::TracingInspectorConfig;
pub use fourbyte::FourByteInspector;

/// An inspector that collects call traces.
///
/// This [Inspector] can be hooked into the [EVM](revm::EVM) which then calls the inspector
/// functions, such as [Inspector::call] or [Inspector::call_end].
///
/// The [TracingInspector] keeps track of everything by:
///   1. start tracking steps/calls on [Inspector::step] and [Inspector::call]
///   2. complete steps/calls on [Inspector::step_end] and [Inspector::call_end]
#[derive(Debug, Clone)]
pub struct TracingInspector {
    /// Configures what and how the inspector records traces.
    config: TracingInspectorConfig,
    /// Records all call traces
    traces: CallTraceArena,
    /// Tracks active calls
    trace_stack: Vec<usize>,
    /// Tracks active steps
    step_stack: Vec<StackStep>,
    /// Tracks the return value of the last call
    last_call_return_data: Option<Bytes>,
    /// The gas inspector used to track remaining gas.
    gas_inspector: GasInspector,
}

// === impl TracingInspector ===

impl TracingInspector {
    /// Returns a new instance for the given config
    pub fn new(config: TracingInspectorConfig) -> Self {
        Self {
            config,
            traces: Default::default(),
            trace_stack: vec![],
            step_stack: vec![],
            last_call_return_data: None,
            gas_inspector: Default::default(),
        }
    }

    /// Consumes the Inspector and returns a [ParityTraceBuilder].
    pub fn into_parity_builder(self) -> ParityTraceBuilder {
        ParityTraceBuilder::new(self.traces.arena, self.config)
    }

    /// Consumes the Inspector and returns a [GethTraceBuilder].
    pub fn into_geth_builder(self) -> GethTraceBuilder {
        GethTraceBuilder::new(self.traces.arena, self.config)
    }

    /// Returns the last trace [CallTrace] index from the stack.
    ///
    /// # Panics
    ///
    /// If no [CallTrace] was pushed
    #[track_caller]
    #[inline]
    fn last_trace_idx(&self) -> usize {
        self.trace_stack.last().copied().expect("can't start step without starting a trace first")
    }

    /// _Removes_ the last trace [CallTrace] index from the stack.
    ///
    /// # Panics
    ///
    /// If no [CallTrace] was pushed
    #[track_caller]
    #[inline]
    fn pop_trace_idx(&mut self) -> usize {
        self.trace_stack.pop().expect("more traces were filled than started")
    }

    /// Starts tracking a new trace.
    ///
    /// Invoked on [Inspector::call].
    fn start_trace_on_call(
        &mut self,
        depth: usize,
        address: Address,
        data: Bytes,
        value: U256,
        kind: CallKind,
        caller: Address,
    ) {
        self.trace_stack.push(self.traces.push_trace(
            0,
            CallTrace {
                depth,
                address,
                kind,
                data,
                value,
                status: InstructionResult::Continue,
                caller,
                last_call_return_value: self.last_call_return_data.clone(),
                ..Default::default()
            },
        ));
    }

    /// Fills the current trace with the outcome of a call.
    ///
    /// Invoked on [Inspector::call_end].
    ///
    /// # Panics
    ///
    /// This expects an existing trace [Self::start_trace_on_call]
    fn fill_trace_on_call_end(
        &mut self,
        status: InstructionResult,
        gas_used: u64,
        output: Bytes,
        created_address: Option<Address>,
    ) {
        let trace_idx = self.pop_trace_idx();
        let trace = &mut self.traces.arena[trace_idx].trace;

        let success = matches!(status, return_ok!());
        trace.status = status;
        trace.success = success;
        trace.gas_used = gas_used;
        trace.output = output.clone();
        self.last_call_return_data = Some(output);

        if let Some(address) = created_address {
            // A new contract was created via CREATE
            trace.address = address;
        }
    }

    /// Starts tracking a step
    ///
    /// Invoked on [Inspector::step]
    ///
    /// # Panics
    ///
    /// This expects an existing [CallTrace], in other words, this panics if not within the context
    /// of a call.
    fn start_step<DB: Database>(&mut self, interp: &mut Interpreter, data: &mut EVMData<'_, DB>) {
        let trace_idx = self.last_trace_idx();
        let trace = &mut self.traces.arena[trace_idx];

        self.step_stack.push(StackStep { trace_idx, step_idx: trace.trace.steps.len() });

        let pc = interp.program_counter();

        let memory =
            self.config.record_memory_snapshots.then(|| interp.memory.clone()).unwrap_or_default();
        let stack =
            self.config.record_stack_snapshots.then(|| interp.stack.clone()).unwrap_or_default();

        trace.trace.steps.push(CallTraceStep {
            depth: data.journaled_state.depth(),
            pc,
            op: OpCode::try_from_u8(interp.contract.bytecode.bytecode()[pc])
                .expect("is valid opcode;"),
            contract: interp.contract.address,
            stack,
            memory,
            memory_size: interp.memory.len(),
            gas: self.gas_inspector.gas_remaining(),
            gas_refund_counter: interp.gas.refunded() as u64,

            // fields will be populated end of call
            gas_cost: 0,
            state_diff: None,
            status: InstructionResult::Continue,
        });
    }

    /// Fills the current trace with the output of a step.
    ///
    /// Invoked on [Inspector::step_end].
    fn fill_step_on_step_end<DB: Database>(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        status: InstructionResult,
    ) {
        let StackStep { trace_idx, step_idx } =
            self.step_stack.pop().expect("can't fill step without starting a step first");
        let step = &mut self.traces.arena[trace_idx].trace.steps[step_idx];

        if let Some(pc) = interp.program_counter().checked_sub(1) {
            if self.config.record_state_diff {
                let op = interp.contract.bytecode.bytecode()[pc];

                let journal_entry = data
                    .journaled_state
                    .journal
                    .last()
                    // This should always work because revm initializes it as `vec![vec![]]`
                    // See [JournaledState::new](revm::JournaledState)
                    .expect("exists; initialized with vec")
                    .last();

                step.state_diff = match (op, journal_entry) {
                    (
                        opcode::SLOAD | opcode::SSTORE,
                        Some(JournalEntry::StorageChange { address, key, .. }),
                    ) => {
                        // SAFETY: (Address,key) exists if part if StorageChange
                        let value =
                            data.journaled_state.state[address].storage[key].present_value();
                        Some((*key, value))
                    }
                    _ => None,
                };
            }

            step.gas_cost = step.gas - self.gas_inspector.gas_remaining();
        }

        // set the status
        step.status = status;
    }
}

impl<DB> Inspector<DB> for TracingInspector
where
    DB: Database,
{
    fn initialize_interp(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
    ) -> InstructionResult {
        self.gas_inspector.initialize_interp(interp, data, is_static)
    }

    fn step(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
    ) -> InstructionResult {
        if self.config.record_steps {
            self.gas_inspector.step(interp, data, is_static);
            self.start_step(interp, data);
        }

        InstructionResult::Continue
    }

    fn log(
        &mut self,
        evm_data: &mut EVMData<'_, DB>,
        address: &Address,
        topics: &[H256],
        data: &Bytes,
    ) {
        self.gas_inspector.log(evm_data, address, topics, data);

        let trace_idx = self.last_trace_idx();
        let trace = &mut self.traces.arena[trace_idx];
        trace.ordering.push(LogCallOrder::Log(trace.logs.len()));
        trace.logs.push(RawLog { topics: topics.to_vec(), data: data.clone() });
    }

    fn step_end(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
        eval: InstructionResult,
    ) -> InstructionResult {
        if self.config.record_steps {
            self.gas_inspector.step_end(interp, data, is_static, eval);
            self.fill_step_on_step_end(interp, data, eval);
            return eval
        }
        InstructionResult::Continue
    }

    fn call(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CallInputs,
        is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        self.gas_inspector.call(data, inputs, is_static);

        // determine correct `from` and `to`  based on the call scheme
        let (from, to) = match inputs.context.scheme {
            CallScheme::DelegateCall | CallScheme::CallCode => {
                (inputs.context.address, inputs.context.code_address)
            }
            _ => (inputs.context.caller, inputs.context.address),
        };

        self.start_trace_on_call(
            data.journaled_state.depth() as usize,
            to,
            inputs.input.clone(),
            inputs.transfer.value,
            inputs.context.scheme.into(),
            from,
        );

        (InstructionResult::Continue, Gas::new(0), Bytes::new())
    }

    fn call_end(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &CallInputs,
        gas: Gas,
        ret: InstructionResult,
        out: Bytes,
        is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        self.gas_inspector.call_end(data, inputs, gas, ret, out.clone(), is_static);

        self.fill_trace_on_call_end(
            ret,
            gas_used(data.env.cfg.spec_id, gas.spend(), gas.refunded() as u64),
            out.clone(),
            None,
        );

        (ret, gas, out)
    }

    fn create(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CreateInputs,
    ) -> (InstructionResult, Option<Address>, Gas, Bytes) {
        self.gas_inspector.create(data, inputs);

        let _ = data.journaled_state.load_account(inputs.caller, data.db);
        let nonce = data.journaled_state.account(inputs.caller).info.nonce;
        self.start_trace_on_call(
            data.journaled_state.depth() as usize,
            get_create_address(inputs, nonce),
            inputs.init_code.clone(),
            inputs.value,
            inputs.scheme.into(),
            inputs.caller,
        );

        (InstructionResult::Continue, None, Gas::new(inputs.gas_limit), Bytes::default())
    }

    /// Called when a contract has been created.
    ///
    /// InstructionResulting anything other than the values passed to this function (`(ret,
    /// remaining_gas, address, out)`) will alter the result of the create.
    fn create_end(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &CreateInputs,
        status: InstructionResult,
        address: Option<Address>,
        gas: Gas,
        retdata: Bytes,
    ) -> (InstructionResult, Option<Address>, Gas, Bytes) {
        self.gas_inspector.create_end(data, inputs, status, address, gas, retdata.clone());

        // get the code of the created contract
        let code = address
            .and_then(|address| {
                data.journaled_state
                    .account(address)
                    .info
                    .code
                    .as_ref()
                    .map(|code| code.bytes()[..code.len()].to_vec())
            })
            .unwrap_or_default();

        self.fill_trace_on_call_end(
            status,
            gas_used(data.env.cfg.spec_id, gas.spend(), gas.refunded() as u64),
            code.into(),
            address,
        );

        (status, address, gas, retdata)
    }

    fn selfdestruct(&mut self, _contract: Address, target: Address) {
        let trace_idx = self.last_trace_idx();
        let trace = &mut self.traces.arena[trace_idx].trace;
        trace.selfdestruct_refund_target = Some(target)
    }
}

#[derive(Debug, Clone, Copy)]
struct StackStep {
    trace_idx: usize,
    step_idx: usize,
}
