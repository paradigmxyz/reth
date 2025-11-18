use alloy_rlp::{RlpDecodable, RlpEncodable};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

use alloy_primitives::{Address, Bytes, U256};

use reth_revm::{
    interpreter::{
        interpreter::EthInterpreter, CallInput, CallInputs, CallOutcome, CreateInputs,
        CreateOutcome, InstructionResult,
    },
    Inspector,
};
use revm_context_interface::{ContextTr, LocalContextTr};

#[derive(Debug, Clone, Default, RlpEncodable, RlpDecodable, Serialize, Deserialize)]
pub struct InternalTransaction {
    dept: u64,
    internal_index: u64,
    call_type: String,
    name: String,
    trace_address: String,
    code_address: String,
    from: String,
    to: String,
    input: Bytes,
    output: Bytes,
    is_error: bool,
    gas: u64,
    gas_used: u64,
    value: String,
    value_wei: String,
    call_value_wei: String,
    error: String,
}

impl InternalTransaction {
    pub fn set_transaction_gas(&mut self, gas_limit: u64, gas_used: u64) {
        self.gas = gas_limit;
        self.gas_used = gas_used;
    }
}

#[derive(Debug, Clone)]
pub struct TraceCollector {
    all_traces: Vec<Vec<InternalTransaction>>,
    traces: Vec<InternalTransaction>,
    // depth
    current_path: Vec<usize>,
    // internal_index
    last_depth: usize,
    sibling_count: Vec<usize>,
    trace_stack: Vec<usize>,
}

impl Default for TraceCollector {
    fn default() -> Self {
        Self {
            all_traces: Vec::<Vec<InternalTransaction>>::default(),
            traces: Vec::<InternalTransaction>::default(),
            current_path: Vec::<usize>::default(),
            last_depth: 0,
            sibling_count: vec![0],
            trace_stack: Vec::<usize>::default(),
        }
    }
}

impl TraceCollector {
    fn format_error(result: &InstructionResult) -> String {
        match result {
            InstructionResult::Revert => "execution reverted".to_string(),
            InstructionResult::CallTooDeep => "max call depth exceeded".to_string(),
            InstructionResult::OutOfGas => "out of gas".to_string(),
            InstructionResult::NonceOverflow => "nonce uint64 overflow".to_string(),
            InstructionResult::InvalidJump => "invalid jump destination".to_string(),
            InstructionResult::CreateCollision => "contract address collision".to_string(),
            InstructionResult::OutOfFunds => "insufficient balance for transfer".to_string(),
            InstructionResult::CreateInitCodeSizeLimit => "max initcode size exceeded".to_string(),
            InstructionResult::OpcodeNotFound => "invalid opcode".to_string(),
            InstructionResult::ReentrancySentryOOG => {
                "not enough gas for reentrancy sentry".to_string()
            }
            InstructionResult::StackUnderflow => "stack underflow".to_string(),
            InstructionResult::StackOverflow => "stack overflow".to_string(),
            InstructionResult::CreateInitCodeStartingEF00 => {
                "CREATE/CREATE2 starts with 0xEF00".to_string()
            }
            InstructionResult::InvalidEOFInitCode => {
                "invalid EVM Object Format (EOF) init code".to_string()
            }
            InstructionResult::InvalidExtDelegateCallTarget => {
                "extDelegateCall calling a non EOF contract".to_string()
            }
            InstructionResult::MemoryOOG => {
                "out of gas error encountered during memory expansion".to_string()
            }
            InstructionResult::MemoryLimitOOG => {
                "the memory limit of the EVM has been exceeded".to_string()
            }
            InstructionResult::PrecompileOOG => {
                "out of gas error encountered during the execution of a precompiled contract"
                    .to_string()
            }
            InstructionResult::InvalidOperandOOG => {
                "out of gas error encountered while calling an invalid operand".to_string()
            }
            InstructionResult::CallNotAllowedInsideStatic => {
                "invalid CALL with value transfer in static context".to_string()
            }
            InstructionResult::StateChangeDuringStaticCall => {
                "invalid state modification in static call".to_string()
            }
            InstructionResult::InvalidFEOpcode => {
                "an undefined bytecode value encountered during execution".to_string()
            }
            InstructionResult::NotActivated => {
                "the feature or opcode is not activated in this version of the EVM".to_string()
            }
            InstructionResult::OutOfOffset => "invalid memory or storage offset".to_string(),
            InstructionResult::OverflowPayment => "payment amount overflow".to_string(),
            InstructionResult::PrecompileError => {
                "error in precompiled contract execution".to_string()
            }
            InstructionResult::CreateContractSizeLimit => {
                "exceeded contract size limit during creation".to_string()
            }
            InstructionResult::CreateContractStartingWithEF => {
                "created contract starts with invalid bytes 0xEF".to_string()
            }
            InstructionResult::FatalExternalError => "fatal external error".to_string(),
            _ => format!("{:?}", result),
        }
    }

    fn init_op(
        &mut self,
        call_type: String,
        from: String,
        to: String,
        input: Bytes,
        value_wei: String,
        gas_limit: u64,
        code_address: String,
    ) {
        let mut txn = InternalTransaction::default();
        txn.call_type = call_type;
        txn.from = from.clone();
        txn.input = input;
        txn.is_error = false;
        txn.gas = gas_limit;
        txn.value_wei = if value_wei.is_empty() { "0" } else { &value_wei }.to_string();
        txn.call_value_wei = match value_wei.parse::<u128>() {
            Ok(value) => format!("0x{:x}", value),
            _ => String::from("0x0"),
        };

        txn.to = to.clone();
        match txn.call_type.as_str() {
            "delegatecall" => {
                txn.from = to;
                txn.to = code_address;
                txn.trace_address = txn.from.clone();
            }
            "callcode" => {
                txn.code_address = code_address;
            }
            _ => {}
        }

        self.traces.push(txn);
    }

    fn before_op(&mut self) {
        let depth = self.current_path.len();

        match depth.cmp(&self.last_depth) {
            Ordering::Greater => {
                self.sibling_count.push(0);
            }
            Ordering::Less => {
                self.sibling_count.truncate(depth + 1);
            }
            Ordering::Equal => {}
        }
        self.last_depth = depth;

        let internal_index = self.sibling_count[depth];
        let trace_index = self.traces.len() - 1;
        self.trace_stack.push(trace_index);

        let txn = &mut self.traces[trace_index];
        txn.dept = depth as u64;
        txn.internal_index = internal_index as u64;

        self.sibling_count[depth] += 1;

        self.current_path.push(internal_index);
        if self.current_path.len() > 1 {
            let suffix =
                self.current_path[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>().join("_");

            txn.name.reserve(1 + suffix.len());
            txn.name.push('_');
            txn.name.push_str(&suffix);
        }

        txn.name = txn.call_type.clone() + &txn.name;
    }

    fn after_op(&mut self) {
        self.current_path.pop();
        if self.trace_stack.is_empty() {
            self.all_traces.push(self.traces.clone());
            self.reset();
        }
    }

    pub fn get(&mut self) -> Vec<Vec<InternalTransaction>> {
        self.all_traces.clone()
    }

    pub fn reset(&mut self) {
        self.traces.clear();
        self.current_path.clear();
        self.last_depth = 0;
        self.sibling_count = vec![0];
        self.trace_stack.clear();
    }
}

impl<CTX> Inspector<CTX, EthInterpreter> for TraceCollector
where
    CTX: ContextTr,
{
    fn call(&mut self, ctx: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let call_type = match inputs.scheme {
            reth_revm::interpreter::CallScheme::Call => "call",
            reth_revm::interpreter::CallScheme::CallCode => "callcode",
            reth_revm::interpreter::CallScheme::DelegateCall => "delegatecall",
            reth_revm::interpreter::CallScheme::StaticCall => "staticcall",
        }
        .to_string();

        let call_input = match &inputs.input {
            CallInput::SharedBuffer(range) => ctx
                .local()
                .shared_memory_buffer_slice(range.clone())
                .map(|s| Bytes::from(s.to_vec()))
                .unwrap_or_default(),
            CallInput::Bytes(b) => b.clone(),
        };

        self.init_op(
            call_type,
            inputs.caller.to_string(),
            inputs.target_address.to_string(),
            call_input,
            inputs.value.get().to_string(),
            inputs.gas_limit,
            inputs.bytecode_address.to_string(),
        );

        self.before_op();

        None
    }

    fn call_end(&mut self, _ctx: &mut CTX, _inputs: &CallInputs, outcome: &mut CallOutcome) {
        let trace_index = self.trace_stack.pop().unwrap_or_default();
        let (_, after) = self.traces.split_at_mut(trace_index);

        if let Some((txn, remainder)) = after.split_first_mut() {
            txn.gas_used = outcome.result.gas.spent();
            txn.output = outcome.result.output.clone();
            txn.is_error = !outcome.result.is_ok();
            txn.error = if txn.is_error {
                Self::format_error(&outcome.result.result)
            } else {
                String::new()
            };
            if txn.is_error {
                for within in remainder {
                    within.is_error = txn.is_error;
                }
            }
        }

        self.after_op();
    }

    fn create(&mut self, _ctx: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        let call_type = match inputs.scheme {
            reth_revm::interpreter::CreateScheme::Create => "create".to_string(),
            reth_revm::interpreter::CreateScheme::Create2 { salt: _ } => "create2".to_string(), /* code_address */
            reth_revm::interpreter::CreateScheme::Custom { address: _ } => "custom".to_string(),
        };

        self.init_op(
            call_type,
            inputs.caller.to_string(),
            "".to_string(),
            inputs.init_code.clone(),
            inputs.value.to_string(),
            inputs.gas_limit,
            "".to_string(),
        );

        self.before_op();

        None
    }

    fn create_end(&mut self, _ctx: &mut CTX, _inputs: &CreateInputs, outcome: &mut CreateOutcome) {
        let trace_index = self.trace_stack.pop().unwrap_or_default();
        let (_, after) = self.traces.split_at_mut(trace_index);

        if let Some((txn, remainder)) = after.split_first_mut() {
            txn.to = outcome.address.unwrap_or_default().to_string();
            txn.gas_used = outcome.result.gas.spent();
            txn.output = outcome.result.output.clone();
            txn.is_error = !outcome.result.is_ok();
            txn.error = if txn.is_error {
                Self::format_error(&outcome.result.result)
            } else {
                String::new()
            };
            if txn.is_error {
                for within in remainder {
                    within.is_error = txn.is_error;
                }
            }
        }

        self.after_op();
    }

    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        self.init_op(
            "selfdestruct".to_string(),
            contract.to_string(),
            target.to_string(),
            Bytes::default(),
            value.to_string(),
            0,
            "".to_string(),
        );

        self.before_op();

        self.trace_stack.pop().unwrap_or_default();

        self.after_op();
    }
}
