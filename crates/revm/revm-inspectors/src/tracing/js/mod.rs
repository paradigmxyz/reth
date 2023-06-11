//! Javascript inspector

use boa_engine::{object::builtins::JsFunction, Context, JsError, JsObject, JsValue, Source};

pub(crate) mod bindings;

/// A javascript inspector that will delegate inspector functions to javascript functions
pub struct JsInspector {
    context: Context<'static>,
    /// The javascript config provided to the inspector.
    config: serde_json::Value,
    /// The evaluated object that contains the inspector functions.
    obj: JsObject,

    result_fn: JsObject,
    fault_fn: JsObject,

    enter_fn: Option<JsObject>,
    exit_fn: Option<JsObject>,
    step_fn: Option<JsObject>,
}

impl JsInspector {
    /// Creates a new inspector from a javascript code snipped that evaluates to an object with the
    /// expected fields and a config object.
    pub fn new(code: String, config: serde_json::Value) -> Result<Self, JsInspectorError> {
        // evaluate the code
        // Instantiate the execution context
        let mut ctx = Context::default();
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
}
