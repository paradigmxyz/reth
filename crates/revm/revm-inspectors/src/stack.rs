use reth_primitives::TxHash;
use revm::{
    inspectors::CustomPrintTracer,
    interpreter::{InstructionResult, Interpreter},
    primitives::Env,
    Database, EVMData, Inspector,
};

/// One can hook on inspector execution in 3 ways:
/// - Block: Hook on block execution
/// - BlockWithIndex: Hook on block execution transaction index
/// - Transaction: Hook on a specific transaction hash
#[derive(Default)]
pub enum Hook {
    #[default]
    /// No hook.
    None,
    /// Hook on a specific block.
    Block(u64),
    /// Hook on a specific transaction hash.
    Transaction(TxHash),
}

#[derive(Default)]
/// An inspector that calls multiple inspectors in sequence.
///
/// If a call to an inspector returns a value other than [InstructionResult::Continue] (or
/// equivalent) the remaining inspectors are not called.
pub struct InspectorStack {
    /// An inspector that prints the opcode traces to the console.
    pub custom_print_tracer: Option<CustomPrintTracer>,
    /// The provided hook
    pub hook: Hook,
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
}
