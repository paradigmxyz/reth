use revm::{
    inspectors::CustomPrintTracer,
    interpreter::{CallInputs, CallOutcome, CreateInputs, CreateOutcome, Interpreter},
    primitives::{Address, Env, Log, B256, U256},
    Database, EvmContext, Inspector,
};
use std::fmt::Debug;

/// One can hook on inspector execution in 3 ways:
/// - Block: Hook on block execution
/// - BlockWithIndex: Hook on block execution transaction index
/// - Transaction: Hook on a specific transaction hash
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hook {
    #[default]
    /// No hook.
    None,
    /// Hook on a specific block.
    Block(u64),
    /// Hook on a specific transaction hash.
    Transaction(B256),
    /// Hooks on every transaction in a block.
    All,
}

/// An inspector that calls multiple inspectors in sequence.
///
/// If a call to an inspector returns a value other than
/// [revm::interpreter::InstructionResult::Continue] (or equivalent) the remaining inspectors are
/// not called.
#[derive(Clone, Default)]
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
        Self {
            hook: config.hook,
            custom_print_tracer: config.use_printer_tracer.then(Default::default),
        }
    }

    /// Check if the inspector should be used.
    pub fn should_inspect(&self, env: &Env, tx_hash: &B256) -> bool {
        match &self.hook {
            Hook::None => false,
            Hook::Block(block) => env.block.number.saturating_to::<u64>() == *block,
            Hook::Transaction(hash) => hash == tx_hash,
            Hook::All => true,
        }
    }
}

/// Configuration for the inspectors.
#[derive(Clone, Copy, Debug, Default)]
pub struct InspectorStackConfig {
    /// Enable revm inspector printer.
    /// In execution this will print opcode level traces directly to console.
    pub use_printer_tracer: bool,

    /// Hook on a specific block or transaction.
    pub hook: Hook,
}

/// Helper macro to call the same method on multiple inspectors without resorting to dynamic
/// dispatch.
#[macro_export]
macro_rules! call_inspectors {
    ([$($inspector:expr),+ $(,)?], |$id:ident $(,)?| $call:expr $(,)?) => {{$(
        if let Some($id) = $inspector {
            $call
        }
    )+}}
}

impl<DB> Inspector<DB> for InspectorStack
where
    DB: Database,
{
    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.initialize_interp(interp, context);
        });
    }

    fn step(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.step(interp, context);
        });
    }

    fn step_end(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.step_end(interp, context);
        });
    }

    fn log(&mut self, context: &mut EvmContext<DB>, log: &Log) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.log(context, log);
        });
    }

    fn call(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            if let Some(outcome) = inspector.call(context, inputs) {
                return Some(outcome);
            }
        });

        None
    }

    fn call_end(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &CallInputs,
        outcome: CallOutcome,
    ) -> CallOutcome {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            let new_ret = inspector.call_end(context, inputs, outcome.clone());

            // If the inspector returns a different ret or a revert with a non-empty message,
            // we assume it wants to tell us something
            if new_ret != outcome {
                return new_ret;
            }
        });

        outcome
    }

    fn create(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut CreateInputs,
    ) -> Option<CreateOutcome> {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            if let Some(out) = inspector.create(context, inputs) {
                return Some(out);
            }
        });

        None
    }

    fn create_end(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &CreateInputs,
        outcome: CreateOutcome,
    ) -> CreateOutcome {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            let new_ret = inspector.create_end(context, inputs, outcome.clone());

            // If the inspector returns a different ret or a revert with a non-empty message,
            // we assume it wants to tell us something
            if new_ret != outcome {
                return new_ret;
            }
        });

        outcome
    }

    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            Inspector::<DB>::selfdestruct(inspector, contract, target, value);
        });
    }
}
