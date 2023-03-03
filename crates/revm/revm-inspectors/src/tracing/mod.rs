use crate::tracing::{
    arena::{CallTrace, CallTraceArena, CallTraceStep, LogCallOrder},
    call::CallKind,
    node::RawLog,
};
use reth_primitives::{bytes::Bytes, Address, H256, U256};
use revm::{
    interpreter::{
        opcode, return_ok, CallInputs, CallScheme, CreateInputs, Gas, InstructionResult,
        Interpreter, OpCode,
    },
    primitives::{CreateScheme, SpecId},
    Database, EVMData, Inspector, JournalEntry,
};

mod arena;
mod call;
mod node;

/// An inspector that collects call traces.
#[derive(Default, Debug, Clone)]
pub struct TracingInspector {
    /// Whether to include individual steps [Inspector::step]
    record_steps: bool,
    /// Records all call traces
    traces: CallTraceArena,
    trace_stack: Vec<usize>,
    step_stack: Vec<StackStep>,
}

// === impl TracingInspector ===

impl TracingInspector {
    fn start_trace(
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
                ..Default::default()
            },
        ));
    }

    fn fill_trace(
        &mut self,
        status: InstructionResult,
        cost: u64,
        output: Bytes,
        address: Option<Address>,
    ) {
        let success = matches!(status, return_ok!());
        let trace = &mut self.traces.arena
            [self.trace_stack.pop().expect("more traces were filled than started")]
        .trace;
        trace.status = status;
        trace.success = success;
        trace.gas_cost = cost;
        trace.output = output;

        if let Some(address) = address {
            trace.address = address;
        }
    }

    fn start_step<DB: Database>(&mut self, interp: &mut Interpreter, data: &mut EVMData<'_, DB>) {
        let trace_idx =
            *self.trace_stack.last().expect("can't start step without starting a trace first");
        let trace = &mut self.traces.arena[trace_idx];

        self.step_stack.push(StackStep { trace_idx, step_idx: trace.trace.steps.len() });

        let pc = interp.program_counter();

        trace.trace.steps.push(CallTraceStep {
            depth: data.journaled_state.depth(),
            pc,
            op: OpCode::try_from_u8(interp.contract.bytecode.bytecode()[pc]).expect("is opcode"),
            contract: interp.contract.address,
            stack: interp.stack.clone(),
            memory: interp.memory.clone(),
            gas: 0,
            // gas: self.gas_inspector.borrow().gas_remaining(),
            gas_refund_counter: interp.gas.refunded() as u64,
            gas_cost: 0,
            state_diff: None,
            error: None,
        });
    }

    fn fill_step<DB: Database>(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        status: InstructionResult,
    ) {
        let StackStep { trace_idx, step_idx } =
            self.step_stack.pop().expect("can't fill step without starting a step first");
        let step = &mut self.traces.arena[trace_idx].trace.steps[step_idx];

        if let Some(pc) = interp.program_counter().checked_sub(1) {
            let op = interp.contract.bytecode.bytecode()[pc];

            let journal_entry = data
                .journaled_state
                .journal
                .last()
                // This should always work because revm initializes it as `vec![vec![]]`
                .unwrap()
                .last();

            step.state_diff = match (op, journal_entry) {
                (
                    opcode::SLOAD | opcode::SSTORE,
                    Some(JournalEntry::StorageChage { address, key, .. }),
                ) => {
                    let value = data.journaled_state.state[address].storage[key].present_value();
                    Some((*key, value))
                }
                _ => None,
            };

            // step.gas_cost = step.gas - self.gas_inspector.borrow().gas_remaining();
        }

        // Error codes only
        if status as u8 > InstructionResult::OutOfGas as u8 {
            step.error = Some(format!("{status:?}"));
        }
    }
}

impl<DB> Inspector<DB> for TracingInspector
where
    DB: Database,
{
    fn step(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        _is_static: bool,
    ) -> InstructionResult {
        if self.record_steps {
            self.start_step(interp, data);
        }

        InstructionResult::Continue
    }

    fn log(
        &mut self,
        _evm_data: &mut EVMData<'_, DB>,
        _address: &Address,
        topics: &[H256],
        data: &Bytes,
    ) {
        let node = &mut self.traces.arena[*self.trace_stack.last().expect("no ongoing trace")];
        node.ordering.push(LogCallOrder::Log(node.logs.len()));
        node.logs.push(RawLog { topics: topics.to_vec(), data: data.clone() });
    }

    fn step_end(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        _is_static: bool,
        eval: InstructionResult,
    ) -> InstructionResult {
        if self.record_steps {
            self.fill_step(interp, data, eval);
            return eval
        }
        InstructionResult::Continue
    }

    fn call(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CallInputs,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        // determine correct `from` and `to`  based on the call scheme
        let (from, to) = match inputs.context.scheme {
            CallScheme::DelegateCall | CallScheme::CallCode => {
                (inputs.context.address, inputs.context.code_address)
            }
            _ => (inputs.context.caller, inputs.context.address),
        };

        self.start_trace(
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
        _inputs: &CallInputs,
        gas: Gas,
        ret: InstructionResult,
        out: Bytes,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        self.fill_trace(
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
        let _ = data.journaled_state.load_account(inputs.caller, data.db);
        let nonce = data.journaled_state.account(inputs.caller).info.nonce;
        self.start_trace(
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
        _inputs: &CreateInputs,
        status: InstructionResult,
        address: Option<Address>,
        gas: Gas,
        retdata: Bytes,
    ) -> (InstructionResult, Option<Address>, Gas, Bytes) {
        let code = match address {
            Some(address) => data
                .journaled_state
                .account(address)
                .info
                .code
                .as_ref()
                .map_or(vec![], |code| code.bytes()[..code.len()].to_vec()),
            None => vec![],
        };
        self.fill_trace(
            status,
            gas_used(data.env.cfg.spec_id, gas.spend(), gas.refunded() as u64),
            code.into(),
            address,
        );

        (status, address, gas, retdata)
    }
}

#[derive(Debug, Clone, Copy)]
struct StackStep {
    trace_idx: usize,
    step_idx: usize,
}

/// Get the gas used, accounting for refunds
pub fn gas_used(spec: SpecId, spent: u64, refunded: u64) -> u64 {
    let refund_quotient = if SpecId::enabled(spec, SpecId::LONDON) { 5 } else { 2 };
    spent - (refunded).min(spent / refund_quotient)
}

/// Get the address of a contract creation
pub fn get_create_address(call: &CreateInputs, nonce: u64) -> Address {
    todo!()
    // match call.scheme {
    //     CreateScheme::Create => get_contract_address(call.caller, nonce),
    //     CreateScheme::Create2 { salt } => {
    //         let mut buffer: [u8; 4 * 8] = [0; 4 * 8];
    //         salt.to_big_endian(&mut buffer);
    //         get_create2_address(call.caller, buffer, call.init_code.clone())
    //     }
    // }
}
