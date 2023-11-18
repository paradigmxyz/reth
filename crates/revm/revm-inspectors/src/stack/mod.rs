use reth_primitives::{Address, Bytes, TxHash, B256, U256};
use revm::{
    inspectors::CustomPrintTracer,
    interpreter::{
        CallInputs, CreateInputs, Gas, InstructionResult, Interpreter, InterpreterResult,
    },
    primitives::Env,
    Database, EvmContext, Inspector,
};
use std::{fmt::Debug, ops::Range};

/// A wrapped [Inspector] that can be reused in the stack
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

/// Configuration for the inspectors.
#[derive(Debug, Default)]
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
    fn initialize_interp(&mut self, interpreter: &mut Interpreter, data: &mut EvmContext<'_, DB>) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            inspector.initialize_interp(interpreter, data);
        });
    }

    fn step(&mut self, interpreter: &mut Interpreter, data: &mut EvmContext<'_, DB>) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            inspector.step(interpreter, data);
        });
    }

    fn log(
        &mut self,
        context: &mut EvmContext<'_, DB>,
        address: &Address,
        topics: &[B256],
        data: &Bytes,
    ) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            inspector.log(context, address, topics, data);
        });
    }

    fn step_end(&mut self, interpreter: &mut Interpreter, data: &mut EvmContext<'_, DB>) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            inspector.step_end(interpreter, data);
        });
    }

    fn call(
        &mut self,
        context: &mut EvmContext<'_, DB>,
        inputs: &mut CallInputs,
    ) -> Option<(InterpreterResult, Range<usize>)> {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            if let Some((interp_result, range)) = inspector.call(context, inputs) {
                // Allow inspectors to exit early
                if interp_result.result != InstructionResult::Continue {
                    return Some((interp_result, range));
                }
            }
        });

        Some((
            InterpreterResult {
                result: InstructionResult::Continue,
                gas: Gas::new(inputs.gas_limit),
                output: Bytes::new(),
            },
            0..0,
        ))
    }

    fn call_end(
        &mut self,
        context: &mut EvmContext<'_, DB>,
        result: InterpreterResult,
    ) -> InterpreterResult {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let new_result = inspector.call_end(context, result.clone());

            // If the inspector returns a different ret or a revert with a non-empty message,
            // we assume it wants to tell us something
            if new_result.result != result.result ||
                (new_result.result == InstructionResult::Revert &&
                    new_result.output != result.output)
            {
                return new_result;
            }
        });

        result
    }

    fn create(
        &mut self,
        context: &mut EvmContext<'_, DB>,
        inputs: &mut CreateInputs,
    ) -> Option<(InterpreterResult, Option<Address>)> {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            // let (status, addr, gas, retdata) = inspector.create(data, inputs);
            if let Some((interp_result, range)) = inspector.create(context, inputs) {
                // Allow inspectors to exit early
                if interp_result.result != InstructionResult::Continue {
                    return Some((interp_result, range));
                }
            }
        });

        Some((
            InterpreterResult {
                result: InstructionResult::Continue,
                gas: Gas::new(inputs.gas_limit),
                output: Bytes::new(),
            },
            None,
        ))
    }

    fn create_end(
        &mut self,
        context: &mut EvmContext<'_, DB>,
        result: InterpreterResult,
        address: Option<Address>,
    ) -> (InterpreterResult, Option<Address>) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            let (new_result, new_address) = inspector.create_end(context, result.clone(), address);

            if new_result.result != result.result {
                return (new_result, new_address);
            }
        });

        (result, address)
    }

    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        call_inspectors!(inspector, [&mut self.custom_print_tracer], {
            Inspector::<DB>::selfdestruct(inspector, contract, target, value);
        });
    }
}
