//! Type bindings for js tracing inspector

use crate::tracing::js::builtins::{to_buf, to_buf_value};
use boa_engine::{
    native_function::NativeFunction, object::FunctionObjectBuilder, Context, JsObject, JsResult,
    JsValue,
};

use boa_gc::{Gc, GcRefCell};
use reth_primitives::{Address, Bytes, H256, U256};
use revm::interpreter::{OpCode, Stack};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;

/// The Log object that is passed to the javascript inspector.
#[derive(Debug)]
pub(crate) struct StepLog {
    /// Stack before step execution
    stack: Stack,
    /// Opcode to be executed
    op: OpCode,
    /// All allocated memory in a step
    memory: Bytes,
    /// Program counter before step execution
    pc: u64,
    /// Remaining gas before step execution
    gas: u64,
    /// Gas cost of step execution
    cost: u64,
    /// Call depth
    depth: usize,
    /// Gas refund counter before step execution
    refund: u64,

    error: Option<String>,
    /// The contract object available to the js inspector
    contract: Contract,
}

impl StepLog {
    /// Converts the contract object into a js object
    ///
    /// Caution: this expects a global property `bigint` to be present.
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        // let Self { stack, op, memory, pc, gas, cost, depth, refund, error, contract } = self;
        let obj = JsObject::default();

        Ok(obj)
    }
}

/// Represents the contract object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Contract {
    caller: Address,
    contract: Address,
    value: U256,
    input: Bytes,
}

impl Contract {
    /// Converts the contract object into a js object
    ///
    /// Caution: this expects a global property `bigint` to be present.
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        let Contract { caller, contract, value, input } = self;
        let obj = JsObject::default();

        let get_caller = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, ctx| {
                to_buf_value(caller.as_bytes().to_vec(), ctx)
            }),
        )
        .length(0)
        .build();

        let get_address = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, ctx| {
                to_buf_value(contract.as_bytes().to_vec(), ctx)
            }),
        )
        .length(0)
        .build();

        let get_value = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, ctx| {
                let bigint = ctx.global_object().get("bigint", ctx)?;
                if !bigint.is_callable() {
                    return Ok(JsValue::undefined())
                }
                bigint.as_callable().unwrap().call(
                    &JsValue::undefined(),
                    &[JsValue::from(value.to_string())],
                    ctx,
                )
            }),
        )
        .length(0)
        .build();

        let input = to_buf_value(input.to_vec(), context)?;

        let get_input = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, _args, input, ctx| Ok(input.clone()),
                input,
            ),
        )
        .length(0)
        .build();

        obj.set("getCaller", get_caller, false, context)?;
        obj.set("getAddress", get_address, false, context)?;
        obj.set("getValue", get_value, false, context)?;
        obj.set("getInput", get_input, false, context)?;

        Ok(obj)
    }
}

/// The `ctx` object that represents the context in which the transaction is executed.
pub(crate) struct EvmContext {
    // TODO more fields
    block_hash: Option<H256>,
    tx_index: Option<usize>,
    tx_hash: Option<H256>,
}

/// DB is the object that allows the js inspector to interact with the database.
pub(crate) struct Db<DB> {
    db: DB,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracing::js::builtins::BIG_INT_JS;
    use boa_engine::{object::builtins::JsArrayBuffer, property::Attribute, Source};

    #[test]
    fn test_contract() {
        let mut ctx = Context::default();
        let contract = Contract {
            caller: Address::zero(),
            contract: Address::zero(),
            value: U256::from(1337u64),
            input: vec![0x01, 0x02, 0x03].into(),
        };
        let big_int = ctx.eval(Source::from_bytes(BIG_INT_JS.as_bytes())).unwrap();
        ctx.register_global_property("bigint", big_int, Attribute::all()).unwrap();

        let obj = contract.clone().into_js_object(&mut ctx).unwrap();
        let s = r#"({
                call: function(contract) { return contract.getCaller(); },
                value: function(contract) { return contract.getValue(); },
                input: function(contract) { return contract.getInput(); }
        })"#;

        let eval_obj = ctx.eval(Source::from_bytes(s.as_bytes())).unwrap();

        let call = eval_obj.as_object().unwrap().get("call", &mut ctx).unwrap();
        let res = call
            .as_callable()
            .unwrap()
            .call(&JsValue::undefined(), &[obj.clone().into()], &mut ctx)
            .unwrap();
        assert!(res.is_object());
        assert!(res.as_object().unwrap().is_array_buffer());

        let call = eval_obj.as_object().unwrap().get("value", &mut ctx).unwrap();
        let res = call
            .as_callable()
            .unwrap()
            .call(&JsValue::undefined(), &[obj.clone().into()], &mut ctx)
            .unwrap();
        assert_eq!(
            res.to_string(&mut ctx).unwrap().to_std_string().unwrap(),
            contract.value.to_string()
        );

        let call = eval_obj.as_object().unwrap().get("input", &mut ctx).unwrap();
        let res = call
            .as_callable()
            .unwrap()
            .call(&JsValue::undefined(), &[obj.into()], &mut ctx)
            .unwrap();

        let buffer = JsArrayBuffer::from_object(res.as_object().unwrap().clone()).unwrap();
        let input = buffer.take().unwrap();
        assert_eq!(input, contract.input);
    }
}
