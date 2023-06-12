//! Javascript inspector

use crate::tracing::js::{
    bindings::{Contract, MemoryObj, OpObj, StackObj, StepLog},
    builtins::register_builtins,
};
use boa_engine::{Context, JsError, JsObject, JsResult, JsValue, Source};
use reth_primitives::bytes::Bytes;
use revm::{
    interpreter::{
        CallInputs, CreateInputs, Gas, InstructionResult, Interpreter, OpCode,
    },
    primitives::B160,
    Database, EVMData, Inspector,
};

pub(crate) mod bindings;
pub(crate) mod builtins;

/// A javascript inspector that will delegate inspector functions to javascript functions
///
/// See also <https://geth.ethereum.org/docs/developers/evm-tracing/custom-tracer#custom-javascript-tracing>
pub struct JsInspector {
    ctx: Context<'static>,
    /// The javascript config provided to the inspector.
    config: JsValue,
    /// The evaluated object that contains the inspector functions.
    obj: JsObject,

    /// The javascript function that will be called when the result is requested.
    result_fn: JsObject,
    fault_fn: JsObject,

    /// EVM inspector hook functions
    enter_fn: Option<JsObject>,
    exit_fn: Option<JsObject>,
    step_fn: Option<JsObject>,
}

impl JsInspector {
    /// Creates a new inspector from a javascript code snipped that evaluates to an object with the
    /// expected fields and a config object.
    ///
    /// The object must have the following fields:
    ///  - `result`: a function that will be called when the result is requested.
    ///  - `fault`: a function that will be called when the transaction fails.
    ///
    /// Optional functions are invoking during inspection:
    /// - `enter`: a function that will be called when the execution enters a new call.
    /// - `exit`: a function that will be called when the execution exits a call.
    /// - `step`: a function that will be called when the execution steps to the next instruction.
    pub fn new(code: String, config: serde_json::Value) -> Result<Self, JsInspectorError> {
        // Instantiate the execution context
        let mut ctx = Context::default();
        register_builtins(&mut ctx)?;

        // evaluate the code
        let code = format!("({})", code);
        let obj =
            ctx.eval(Source::from_bytes(code.as_bytes())).map_err(JsInspectorError::EvalCode)?;

        let obj = obj.as_object().cloned().ok_or(JsInspectorError::ExpectedJsObject)?;

        // ensure all the fields are callables, if present

        let result_fn = obj
            .get("result", &mut ctx)?
            .as_object()
            .cloned()
            .ok_or(JsInspectorError::ResultFunctionMissing)?;
        if !result_fn.is_callable() {
            return Err(JsInspectorError::ResultFunctionMissing)
        }

        let fault_fn = obj
            .get("fault", &mut ctx)?
            .as_object()
            .cloned()
            .ok_or(JsInspectorError::ResultFunctionMissing)?;
        if !result_fn.is_callable() {
            return Err(JsInspectorError::ResultFunctionMissing)
        }

        let enter_fn = obj.get("enter", &mut ctx)?.as_object().cloned().filter(|o| o.is_callable());
        let exit_fn = obj.get("exit", &mut ctx)?.as_object().cloned().filter(|o| o.is_callable());
        let step_fn = obj.get("step", &mut ctx)?.as_object().cloned().filter(|o| o.is_callable());

        let config =
            JsValue::from_json(&config, &mut ctx).map_err(JsInspectorError::InvalidJsonConfig)?;

        if let Some(setup_fn) = obj.get("setup", &mut ctx)?.as_object() {
            if !setup_fn.is_callable() {
                return Err(JsInspectorError::SetupFunctionNotCallable)
            }

            // call setup()
            setup_fn
                .call(&(obj.clone().into()), &[config.clone()], &mut ctx)
                .map_err(JsInspectorError::SetupCallFailed)?;
        }

        Ok(Self { ctx, config, obj, result_fn, fault_fn, enter_fn, exit_fn, step_fn })
    }

    /// Calls the result function and returns the result as [serde_json::Value].
    ///
    /// Note: This is supposed to be called after the inspection has finished.
    pub fn json_result(&mut self) -> JsResult<serde_json::Value> {
        self.result()?.to_json(&mut self.ctx)
    }

    /// Calls the result function and returns the result.
    pub fn result(&mut self) -> JsResult<JsValue> {
        self.result_fn.call(&(self.obj.clone().into()), &[], &mut self.ctx)
    }

    fn try_step(&mut self, step: StepLog) -> JsResult<()> {
        if let Some(step_fn) = &self.step_fn {
            let val = step.into_js_object(&mut self.ctx)?;
            step_fn.call(&(self.obj.clone().into()), &[val.into()], &mut self.ctx)?;
        }
        Ok(())
    }

    fn try_enter(&mut self) -> JsResult<()> {
        if let Some(enter_fn) = &self.enter_fn {
            enter_fn.call(&(self.obj.clone().into()), &[], &mut self.ctx)?;
        }
        Ok(())
    }

    fn try_exit(&mut self) -> JsResult<()> {
        if let Some(exit_fn) = &self.exit_fn {
            exit_fn.call(&(self.obj.clone().into()), &[], &mut self.ctx)?;
        }
        Ok(())
    }
}

impl<DB> Inspector<DB> for JsInspector
where
    DB: Database,
{
    fn initialize_interp(
        &mut self,
        _interp: &mut Interpreter,
        _data: &mut EVMData<'_, DB>,
        _is_static: bool,
    ) -> InstructionResult {
        todo!()
    }

    fn step(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        _is_static: bool,
    ) -> InstructionResult {
        if self.step_fn.is_none() {
            return InstructionResult::Continue
        }

        let pc = interp.program_counter();
        let step = StepLog {
            stack: StackObj(interp.stack.clone()),
            op: OpObj(
                OpCode::try_from_u8(interp.contract.bytecode.bytecode()[pc])
                    .expect("is valid opcode;"),
            ),
            memory: MemoryObj(interp.memory.clone()),
            pc: pc as u64,
            gas: 0,
            cost: 0,
            depth: data.journaled_state.depth(),
            refund: 0,
            error: None,
            // TODO need to track this in a stack via call/call_end
            contract: Contract {
                caller: Default::default(),
                contract: Default::default(),
                value: Default::default(),
                input: Default::default(),
            },
        };

        if self.try_step(step).is_err() {
            return InstructionResult::Revert
        }
        InstructionResult::Continue
    }

    fn step_end(
        &mut self,
        _interp: &mut Interpreter,
        _data: &mut EVMData<'_, DB>,
        _is_static: bool,
        _eval: InstructionResult,
    ) -> InstructionResult {
        InstructionResult::Continue
    }

    fn call(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        _inputs: &mut CallInputs,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        todo!()
    }

    fn call_end(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        _inputs: &CallInputs,
        _remaining_gas: Gas,
        _ret: InstructionResult,
        _out: Bytes,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        todo!()
    }

    fn create(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        _inputs: &mut CreateInputs,
    ) -> (InstructionResult, Option<B160>, Gas, Bytes) {
        todo!()
    }

    fn create_end(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        _inputs: &CreateInputs,
        _ret: InstructionResult,
        _address: Option<B160>,
        _remaining_gas: Gas,
        _out: Bytes,
    ) -> (InstructionResult, Option<B160>, Gas, Bytes) {
        todo!()
    }

    fn selfdestruct(&mut self, _contract: B160, _target: B160) {
        // capture enter
        todo!()
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum JsInspectorError {
    #[error(transparent)]
    JsError(#[from] JsError),
    #[error("Failed to eval js code: {0}")]
    EvalCode(JsError),
    #[error("The evaluated code is not a JS object")]
    ExpectedJsObject,
    #[error("trace object must expose a function result()")]
    ResultFunctionMissing,
    #[error("trace object must expose a function fault()")]
    FaultFunctionMissing,
    #[error("setup object must be a function")]
    SetupFunctionNotCallable,
    #[error("Failed to call setup(): {0}")]
    SetupCallFailed(JsError),
    #[error("Invalid JSON config: {0}")]
    InvalidJsonConfig(JsError),
}
