use revm::{
    inspectors::CustomPrintTracer,
    interpreter::{CallInputs, CallOutcome, CreateInputs, CreateOutcome, Interpreter},
    primitives::{Address, Env, Log, B256, U256},
    Database, EvmContext, Inspector,
};
use std::fmt::Debug;

/// A hook to inspect the execution of the EVM.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hook {
    /// No hook.
    #[default]
    None,
    /// Hook on a specific block.
    Block(u64),
    /// Hook on a specific transaction hash.
    Transaction(B256),
    /// Hooks on every transaction in a block.
    All,
}

impl Hook {
    /// Returns `true` if this hook should be used.
    #[inline]
    pub fn is_enabled(&self, block_number: u64, tx_hash: &B256) -> bool {
        match self {
            Hook::None => false,
            Hook::Block(block) => block_number == *block,
            Hook::Transaction(hash) => hash == tx_hash,
            Hook::All => true,
        }
    }
}

/// An inspector that calls multiple inspectors in sequence.
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
    /// Creates a new inspector stack with the given configuration.
    #[inline]
    pub fn new(config: InspectorStackConfig) -> Self {
        Self {
            hook: config.hook,
            custom_print_tracer: config.use_printer_tracer.then(Default::default),
        }
    }

    /// Returns `true` if this inspector should be used.
    #[inline]
    pub fn should_inspect(&self, env: &Env, tx_hash: &B256) -> bool {
        self.custom_print_tracer.is_some() &&
            self.hook.is_enabled(env.block.number.saturating_to(), tx_hash)
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
    #[inline]
    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.initialize_interp(interp, context);
        });
    }

    #[inline]
    fn step(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.step(interp, context);
        });
    }

    #[inline]
    fn step_end(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.step_end(interp, context);
        });
    }

    #[inline]
    fn log(&mut self, context: &mut EvmContext<DB>, log: &Log) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            inspector.log(context, log);
        });
    }

    #[inline]
    fn call(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            if let Some(outcome) = inspector.call(context, inputs) {
                return Some(outcome)
            }
        });

        None
    }

    #[inline]
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
                return new_ret
            }
        });

        outcome
    }

    #[inline]
    fn create(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut CreateInputs,
    ) -> Option<CreateOutcome> {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            if let Some(out) = inspector.create(context, inputs) {
                return Some(out)
            }
        });

        None
    }

    #[inline]
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
                return new_ret
            }
        });

        outcome
    }

    #[inline]
    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        call_inspectors!([&mut self.custom_print_tracer], |inspector| {
            Inspector::<DB>::selfdestruct(inspector, contract, target, value);
        });
    }
}
