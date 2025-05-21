use super::precompiles::BscPrecompiles;
use revm::{
    context::{ContextSetters, Evm as EvmCtx},
    context_interface::ContextTr,
    handler::{
        instructions::{EthInstructions, InstructionProvider},
        EvmTr, PrecompileProvider,
    },
    inspector::{InspectorEvmTr, JournalExt},
    interpreter::{interpreter::EthInterpreter, Interpreter, InterpreterAction, InterpreterTypes},
    Inspector,
};

pub mod builder;
pub mod ctx;
mod exec;

pub struct BscEvmInner<CTX, INSP, I = EthInstructions<EthInterpreter, CTX>, P = BscPrecompiles>(
    pub EvmCtx<CTX, INSP, I, P>,
);

impl<CTX: ContextTr, INSP>
    BscEvmInner<CTX, INSP, EthInstructions<EthInterpreter, CTX>, BscPrecompiles>
{
    pub fn new(ctx: CTX, inspector: INSP) -> Self {
        Self(EvmCtx {
            ctx,
            inspector,
            instruction: EthInstructions::new_mainnet(),
            precompiles: BscPrecompiles::default(),
        })
    }

    /// Consumes self and returns a new Evm type with given Precompiles.
    pub fn with_precompiles<OP>(
        self,
        precompiles: OP,
    ) -> BscEvmInner<CTX, INSP, EthInstructions<EthInterpreter, CTX>, OP> {
        BscEvmInner(self.0.with_precompiles(precompiles))
    }
}

impl<CTX, INSP, I, P> InspectorEvmTr for BscEvmInner<CTX, INSP, I, P>
where
    CTX: ContextTr<Journal: JournalExt> + ContextSetters,
    I: InstructionProvider<
        Context = CTX,
        InterpreterTypes: InterpreterTypes<Output = InterpreterAction>,
    >,
    INSP: Inspector<CTX, I::InterpreterTypes>,
    P: PrecompileProvider<CTX>,
{
    type Inspector = INSP;

    fn inspector(&mut self) -> &mut Self::Inspector {
        &mut self.0.inspector
    }

    fn ctx_inspector(&mut self) -> (&mut Self::Context, &mut Self::Inspector) {
        (&mut self.0.ctx, &mut self.0.inspector)
    }

    fn run_inspect_interpreter(
        &mut self,
        interpreter: &mut Interpreter<
            <Self::Instructions as InstructionProvider>::InterpreterTypes,
        >,
    ) -> <<Self::Instructions as InstructionProvider>::InterpreterTypes as InterpreterTypes>::Output
    {
        self.0.run_inspect_interpreter(interpreter)
    }
}

impl<CTX, INSP, I, P> EvmTr for BscEvmInner<CTX, INSP, I, P>
where
    CTX: ContextTr,
    I: InstructionProvider<
        Context = CTX,
        InterpreterTypes: InterpreterTypes<Output = InterpreterAction>,
    >,
    P: PrecompileProvider<CTX>,
{
    type Context = CTX;
    type Instructions = I;
    type Precompiles = P;

    fn run_interpreter(
        &mut self,
        interpreter: &mut Interpreter<
            <Self::Instructions as InstructionProvider>::InterpreterTypes,
        >,
    ) -> <<Self::Instructions as InstructionProvider>::InterpreterTypes as InterpreterTypes>::Output
    {
        let context = &mut self.0.ctx;
        let instructions = &mut self.0.instruction;
        interpreter.run_plain(instructions.instruction_table(), context)
    }

    fn ctx(&mut self) -> &mut Self::Context {
        &mut self.0.ctx
    }

    fn ctx_ref(&self) -> &Self::Context {
        &self.0.ctx
    }

    fn ctx_instructions(&mut self) -> (&mut Self::Context, &mut Self::Instructions) {
        (&mut self.0.ctx, &mut self.0.instruction)
    }

    fn ctx_precompiles(&mut self) -> (&mut Self::Context, &mut Self::Precompiles) {
        (&mut self.0.ctx, &mut self.0.precompiles)
    }
}

#[cfg(test)]
mod test {
    use super::{builder::BscBuilder, ctx::DefaultBsc};
    use revm::{
        inspector::{InspectEvm, NoOpInspector},
        Context, ExecuteEvm,
    };

    #[test]
    fn default_run_bsc() {
        let ctx = Context::bsc();
        let mut evm = ctx.build_bsc_with_inspector(NoOpInspector {});

        // execute
        let _ = evm.replay();
        // inspect
        let _ = evm.inspect_replay();
    }
}
