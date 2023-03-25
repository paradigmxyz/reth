use std::fmt::Debug;

use reth_primitives::{bytes::Bytes, Address, TxHash, H256};
use revm::{
    inspectors::CustomPrintTracer,
    interpreter::{CallInputs, CreateInputs, Gas, InstructionResult, Interpreter},
    primitives::Env,
    Database, EVMData, Inspector,
};

/// A wrapped [Inspector](revm::Inspector) that can be reused in the stack
mod maybe_owned;
pub use maybe_owned::MaybeOwnedInspector;

/// One can hook on inspector execution in 3 ways:
/// - Block: Hook on block execution
/// - BlockWithIndex: Hook on block execution transaction index
/// - Transaction: Hook on a specific transaction hash
#[derive(Debug, Default, Clone)]
pub enum Hook {
    #[default]
    /// No hook.
    None,
    /// Hook on a specific block.
    Block(u64),
    /// Hook on a specific transaction hash.
    Transaction(TxHash),
    /// Hooks on every transaction in a block.
    All,
}

/// An inspector that calls multiple inspectors in sequence.
///
/// If a call to an inspector returns a value other than [InstructionResult::Continue] (or
/// equivalent) the remaining inspectors are not called.
#[derive(Default, Clone)]
pub struct InspectorStack {
    /// An inspector that prints the opcode traces to the console.
    pub custom_print_tracer: Option<CustomPrintTracer>,
    /// The provided hook
    pub hook: Hook,
}

impl Debug for InspectorStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectorStack")
            .field("custom_print_tracer", &self.custom_print_tracer.is_some())
            .field("hook", &self.hook)
            .finish()
    }
}

impl InspectorStack {
    /// Create a new inspector stack.
    pub fn new(config: InspectorStackConfig) -> Self {
        let mut stack = InspectorStack { hook: config.hook, ..Default::default() };

        if config.use_printer_tracer {
            stack.custom_print_tracer = Some(CustomPrintTracer::default());
        }

        stack
    }

    /// Check if the inspector should be used.
    pub fn should_inspect(&self, env: &Env, tx_hash: TxHash) -> bool {
        match self.hook {
            Hook::None => false,
            Hook::Block(block) => env.block.number.to::<u64>() == block,
            Hook::Transaction(hash) => hash == tx_hash,
            Hook::All => true,
        }
    }
}

#[derive(Default)]
/// Configuration for the inspectors.
pub struct InspectorStackConfig {
    /// Enable revm inspector printer.
    /// In execution this will print opcode level traces directly to console.
    pub use_printer_tracer: bool,

    /// Hook on a specific block or transaction.
    pub hook: Hook,
}

/// Helper macro to call the same method on multiple inspectors without resorting to dynamic
/// dispatch
#[macro_export]
macro_rules! call_inspectors {
    ($id:ident, [ $($inspector:expr),+ ], $call:block) => {
        $({
            if let Some($id) = $inspector {
                $call;
            }
        })+
    }
}

impl<DB> Inspector<DB> for InspectorStack
where
    DB: Database,
{
    fn initialize_interp(
        &mut self,
        interpreter: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
    ) -> InstructionResult {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let status = inspector.initialize_interp(interpreter, data, is_static);

            // Allow inspectors to exit early
            if status != InstructionResult::Continue {
                return status
            }
        });

        InstructionResult::Continue
    }

    fn step(
        &mut self,
        interpreter: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
    ) -> InstructionResult {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let status = inspector.step(interpreter, data, is_static);

            // Allow inspectors to exit early
            if status != InstructionResult::Continue {
                return status
            }
        });

        InstructionResult::Continue
    }

    fn log(
        &mut self,
        evm_data: &mut EVMData<'_, DB>,
        address: &Address,
        topics: &[H256],
        data: &Bytes,
    ) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            inspector.log(evm_data, address, topics, data);
        });
    }

    fn step_end(
        &mut self,
        interpreter: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
        eval: InstructionResult,
    ) -> InstructionResult {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let status = inspector.step_end(interpreter, data, is_static, eval);

            // Allow inspectors to exit early
            if status != InstructionResult::Continue {
                return status
            }
        });

        InstructionResult::Continue
    }

    fn call(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CallInputs,
        is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let (status, gas, retdata) = inspector.call(data, inputs, is_static);

            // Allow inspectors to exit early
            if status != InstructionResult::Continue {
                return (status, gas, retdata)
            }
        });

        (InstructionResult::Continue, Gas::new(inputs.gas_limit), Bytes::new())
    }

    fn call_end(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &CallInputs,
        remaining_gas: Gas,
        ret: InstructionResult,
        out: Bytes,
        is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let (new_ret, new_gas, new_out) =
                inspector.call_end(data, inputs, remaining_gas, ret, out.clone(), is_static);

            // If the inspector returns a different ret or a revert with a non-empty message,
            // we assume it wants to tell us something
            if new_ret != ret || (new_ret == InstructionResult::Revert && new_out != out) {
                return (new_ret, new_gas, new_out)
            }
        });

        (ret, remaining_gas, out)
    }

    fn create(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CreateInputs,
    ) -> (InstructionResult, Option<Address>, Gas, Bytes) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let (status, addr, gas, retdata) = inspector.create(data, inputs);

            // Allow inspectors to exit early
            if status != InstructionResult::Continue {
                return (status, addr, gas, retdata)
            }
        });

        (InstructionResult::Continue, None, Gas::new(inputs.gas_limit), Bytes::new())
    }

    fn create_end(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &CreateInputs,
        ret: InstructionResult,
        address: Option<Address>,
        remaining_gas: Gas,
        out: Bytes,
    ) -> (InstructionResult, Option<Address>, Gas, Bytes) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let (new_ret, new_address, new_gas, new_retdata) =
                inspector.create_end(data, inputs, ret, address, remaining_gas, out.clone());

            if new_ret != ret {
                return (new_ret, new_address, new_gas, new_retdata)
            }
        });

        (ret, address, remaining_gas, out)
    }

    fn selfdestruct(&mut self, contract: Address, target: Address) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            Inspector::<DB>::selfdestruct(inspector, contract, target);
        });
    }
}
