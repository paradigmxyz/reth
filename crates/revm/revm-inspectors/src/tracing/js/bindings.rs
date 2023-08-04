//! Type bindings for js tracing inspector

use crate::tracing::{
    js::{
        builtins::{
            address_to_buf, bytes_to_address, bytes_to_hash, from_buf, to_bigint, to_bigint_array,
            to_buf, to_buf_value,
        },
        JsDbRequest,
    },
    types::CallKind,
};
use boa_engine::{
    native_function::NativeFunction,
    object::{builtins::JsArrayBuffer, FunctionObjectBuilder},
    Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsValue,
};
use boa_gc::{empty_trace, Finalize, Gc, Trace};
use reth_primitives::{bytes::Bytes, Account, Address, H256, KECCAK_EMPTY, U256};
use revm::{
    interpreter::{
        opcode::{PUSH0, PUSH32},
        Memory, OpCode, Stack,
    },
    primitives::State,
};
use std::{borrow::Borrow, sync::mpsc::channel};
use tokio::sync::mpsc;

/// A macro that creates a native function that returns via [JsValue::from]
macro_rules! js_value_getter {
    ($value:ident, $ctx:ident) => {
        FunctionObjectBuilder::new(
            $ctx,
            NativeFunction::from_copy_closure(move |_this, _args, _ctx| Ok(JsValue::from($value))),
        )
        .length(0)
        .build()
    };
}

/// A macro that creates a native function that returns a captured JsValue
macro_rules! js_value_capture_getter {
    ($value:ident, $ctx:ident) => {
        FunctionObjectBuilder::new(
            $ctx,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, _args, input, _ctx| Ok(JsValue::from(input.clone())),
                $value,
            ),
        )
        .length(0)
        .build()
    };
}

/// The Log object that is passed to the javascript inspector.
#[derive(Debug)]
pub(crate) struct StepLog {
    /// Stack before step execution
    pub(crate) stack: StackObj,
    /// Opcode to be executed
    pub(crate) op: OpObj,
    /// All allocated memory in a step
    pub(crate) memory: MemoryObj,
    /// Program counter before step execution
    pub(crate) pc: u64,
    /// Remaining gas before step execution
    pub(crate) gas_remaining: u64,
    /// Gas cost of step execution
    pub(crate) cost: u64,
    /// Call depth
    pub(crate) depth: u64,
    /// Gas refund counter before step execution
    pub(crate) refund: u64,
    /// returns information about the error if one occurred, otherwise returns undefined
    pub(crate) error: Option<String>,
    /// The contract object available to the js inspector
    pub(crate) contract: Contract,
}

impl StepLog {
    /// Converts the contract object into a js object
    ///
    /// Caution: this expects a global property `bigint` to be present.
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        let Self {
            stack,
            op,
            memory,
            pc,
            gas_remaining: gas,
            cost,
            depth,
            refund,
            error,
            contract,
        } = self;
        let obj = JsObject::default();

        // fields
        let op = op.into_js_object(context)?;
        let memory = memory.into_js_object(context)?;
        let stack = stack.into_js_object(context)?;
        let contract = contract.into_js_object(context)?;

        obj.set("op", op, false, context)?;
        obj.set("memory", memory, false, context)?;
        obj.set("stack", stack, false, context)?;
        obj.set("contract", contract, false, context)?;

        // methods
        let error =
            if let Some(error) = error { JsValue::from(error) } else { JsValue::undefined() };
        let get_error = js_value_capture_getter!(error, context);
        let get_pc = js_value_getter!(pc, context);
        let get_gas = js_value_getter!(gas, context);
        let get_cost = js_value_getter!(cost, context);
        let get_refund = js_value_getter!(refund, context);
        let get_depth = js_value_getter!(depth, context);

        obj.set("getPc", get_pc, false, context)?;
        obj.set("getError", get_error, false, context)?;
        obj.set("getGas", get_gas, false, context)?;
        obj.set("getCost", get_cost, false, context)?;
        obj.set("getDepth", get_depth, false, context)?;
        obj.set("getRefund", get_refund, false, context)?;

        Ok(obj)
    }
}

/// Represents the memory object
#[derive(Debug)]
pub(crate) struct MemoryObj(pub(crate) Memory);

impl MemoryObj {
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        let obj = JsObject::default();
        let len = self.0.len();
        // TODO: add into data <https://github.com/bluealloy/revm/pull/516>
        let value = to_buf(self.0.data().clone(), context)?;

        let length = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, _ctx| {
                Ok(JsValue::from(len as u64))
            }),
        )
        .length(0)
        .build();

        let slice = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                |_this, args, memory, ctx| {
                    let start = args.get_or_undefined(0).to_number(ctx)?;
                    let end = args.get_or_undefined(1).to_number(ctx)?;
                    if end < start || start < 0. {
                        return Err(JsError::from_native(JsNativeError::typ().with_message(
                            format!(
                                "tracer accessed out of bound memory: offset {start}, end {end}"
                            ),
                        )))
                    }
                    let start = start as usize;
                    let end = end as usize;

                    let mut mem = memory.take()?;
                    let slice = mem.drain(start..end).collect::<Vec<u8>>();
                    to_buf_value(slice, ctx)
                },
                value.clone(),
            ),
        )
        .length(2)
        .build();

        let get_uint = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                 |_this, args, memory, ctx|  {
                    let offset_f64 = args.get_or_undefined(0).to_number(ctx)?;

                     let mut mem = memory.take()?;
                     let offset = offset_f64 as usize;
                     if mem.len() < offset+32 || offset_f64 < 0. {
                         return Err(JsError::from_native(
                             JsNativeError::typ().with_message(format!("tracer accessed out of bound memory: available {}, offset {}, size 32", mem.len(), offset))
                         ));
                     }

                    let slice = mem.drain(offset..offset+32).collect::<Vec<u8>>();
                     to_buf_value(slice, ctx)
                },
                value
            ),
        )
            .length(1)
            .build();

        obj.set("slice", slice, false, context)?;
        obj.set("getUint", get_uint, false, context)?;
        obj.set("length", length, false, context)?;
        Ok(obj)
    }
}

/// Represents the opcode object
#[derive(Debug)]
pub(crate) struct OpObj(pub(crate) u8);

impl OpObj {
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        let obj = JsObject::default();
        let value = self.0;
        let is_push = (PUSH0..=PUSH32).contains(&value);

        let to_number = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, _ctx| Ok(JsValue::from(value))),
        )
        .length(0)
        .build();

        let is_push = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, _ctx| Ok(JsValue::from(is_push))),
        )
        .length(0)
        .build();

        let to_string = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, _ctx| {
                let op = OpCode::try_from_u8(value)
                    .or_else(|| {
                        // if the opcode is invalid, we'll use the invalid opcode to represent it
                        // because this is invoked before the opcode is
                        // executed, the evm will eventually return a `Halt`
                        // with invalid/unknown opcode as result
                        let invalid_opcode = 0xfe;
                        OpCode::try_from_u8(invalid_opcode)
                    })
                    .expect("is valid opcode;");
                let s = op.to_string();
                Ok(JsValue::from(s))
            }),
        )
        .length(0)
        .build();

        obj.set("toNumber", to_number, false, context)?;
        obj.set("toString", to_string, false, context)?;
        obj.set("isPush", is_push, false, context)?;
        Ok(obj)
    }
}

impl From<u8> for OpObj {
    fn from(op: u8) -> Self {
        Self(op)
    }
}

/// Represents the stack object
#[derive(Debug, Clone)]
pub(crate) struct StackObj(pub(crate) Stack);

impl StackObj {
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        let obj = JsObject::default();
        let stack = self.0;
        let len = stack.len();
        let stack_arr = to_bigint_array(stack.data(), context)?;
        let length = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure(move |_this, _args, _ctx| Ok(JsValue::from(len))),
        )
        .length(0)
        .build();

        let peek = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, args, stack_arr, ctx| {
                    let idx_f64 = args.get_or_undefined(0).to_number(ctx)?;
                    let idx = idx_f64 as usize;
                    if len <= idx || idx_f64 < 0. {
                        return Err(JsError::from_native(JsNativeError::typ().with_message(
                            format!(
                                "tracer accessed out of bound stack: size {len}, index {idx_f64}"
                            ),
                        )))
                    }
                    stack_arr.get(idx as u64, ctx)
                },
                stack_arr,
            ),
        )
        .length(1)
        .build();

        obj.set("length", length, false, context)?;
        obj.set("peek", peek, false, context)?;
        Ok(obj)
    }
}

/// Represents the contract object
#[derive(Debug, Clone, Default)]
pub(crate) struct Contract {
    pub(crate) caller: Address,
    pub(crate) contract: Address,
    pub(crate) value: U256,
    pub(crate) input: Bytes,
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
            NativeFunction::from_copy_closure(move |_this, _args, ctx| to_bigint(value, ctx)),
        )
        .length(0)
        .build();

        let input = to_buf_value(input.to_vec(), context)?;
        let get_input = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, _args, input, _ctx| Ok(input.clone()),
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

/// Represents the call frame object for exit functions
pub(crate) struct FrameResult {
    pub(crate) gas_used: u64,
    pub(crate) output: Bytes,
    pub(crate) error: Option<String>,
}

impl FrameResult {
    pub(crate) fn into_js_object(self, ctx: &mut Context<'_>) -> JsResult<JsObject> {
        let Self { gas_used, output, error } = self;
        let obj = JsObject::default();

        let output = to_buf_value(output.to_vec(), ctx)?;
        let get_output = FunctionObjectBuilder::new(
            ctx,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, _args, output, _ctx| Ok(output.clone()),
                output,
            ),
        )
        .length(0)
        .build();

        let error = error.map(JsValue::from).unwrap_or_default();
        let get_error = js_value_capture_getter!(error, ctx);
        let get_gas_used = js_value_getter!(gas_used, ctx);

        obj.set("getGasUsed", get_gas_used, false, ctx)?;
        obj.set("getOutput", get_output, false, ctx)?;
        obj.set("getError", get_error, false, ctx)?;

        Ok(obj)
    }
}

/// Represents the call frame object for enter functions
pub(crate) struct CallFrame {
    pub(crate) contract: Contract,
    pub(crate) kind: CallKind,
    pub(crate) gas: u64,
}

impl CallFrame {
    pub(crate) fn into_js_object(self, ctx: &mut Context<'_>) -> JsResult<JsObject> {
        let CallFrame { contract: Contract { caller, contract, value, input }, kind, gas } = self;
        let obj = JsObject::default();

        let get_from = FunctionObjectBuilder::new(
            ctx,
            NativeFunction::from_copy_closure(move |_this, _args, ctx| {
                to_buf_value(caller.as_bytes().to_vec(), ctx)
            }),
        )
        .length(0)
        .build();

        let get_to = FunctionObjectBuilder::new(
            ctx,
            NativeFunction::from_copy_closure(move |_this, _args, ctx| {
                to_buf_value(contract.as_bytes().to_vec(), ctx)
            }),
        )
        .length(0)
        .build();

        let get_value = FunctionObjectBuilder::new(
            ctx,
            NativeFunction::from_copy_closure(move |_this, _args, ctx| to_bigint(value, ctx)),
        )
        .length(0)
        .build();

        let input = to_buf_value(input.to_vec(), ctx)?;
        let get_input = FunctionObjectBuilder::new(
            ctx,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, _args, input, _ctx| Ok(input.clone()),
                input,
            ),
        )
        .length(0)
        .build();

        let get_gas = js_value_getter!(gas, ctx);
        let ty = kind.to_string();
        let get_type = js_value_capture_getter!(ty, ctx);

        obj.set("getFrom", get_from, false, ctx)?;
        obj.set("getTo", get_to, false, ctx)?;
        obj.set("getValue", get_value, false, ctx)?;
        obj.set("getInput", get_input, false, ctx)?;
        obj.set("getGas", get_gas, false, ctx)?;
        obj.set("getType", get_type, false, ctx)?;

        Ok(obj)
    }
}

/// The `ctx` object that represents the context in which the transaction is executed.
pub(crate) struct EvmContext {
    /// String, one of the two values CALL and CREATE
    pub(crate) r#type: String,
    /// Sender of the transaction
    pub(crate) from: Address,
    /// Target of the transaction
    pub(crate) to: Option<Address>,
    pub(crate) input: Bytes,
    /// Gas limit
    pub(crate) gas: u64,
    /// Number, amount of gas used in executing the transaction (excludes txdata costs)
    pub(crate) gas_used: u64,
    /// Number, gas price configured in the transaction being executed
    pub(crate) gas_price: u64,
    /// Number, intrinsic gas for the transaction being executed
    pub(crate) intrinsic_gas: u64,
    /// big.int Amount to be transferred in wei
    pub(crate) value: U256,
    /// Number, block number
    pub(crate) block: u64,
    pub(crate) output: Bytes,
    /// Number, block number
    pub(crate) time: String,
    pub(crate) block_hash: Option<H256>,
    pub(crate) tx_index: Option<usize>,
    pub(crate) tx_hash: Option<H256>,
}

impl EvmContext {
    pub(crate) fn into_js_object(self, ctx: &mut Context<'_>) -> JsResult<JsObject> {
        let Self {
            r#type,
            from,
            to,
            input,
            gas,
            gas_used,
            gas_price,
            intrinsic_gas,
            value,
            block,
            output,
            time,
            block_hash,
            tx_index,
            tx_hash,
        } = self;
        let obj = JsObject::default();

        // add properties

        obj.set("type", r#type, false, ctx)?;
        obj.set("from", address_to_buf(from, ctx)?, false, ctx)?;
        if let Some(to) = to {
            obj.set("to", address_to_buf(to, ctx)?, false, ctx)?;
        } else {
            obj.set("to", JsValue::null(), false, ctx)?;
        }

        obj.set("input", to_buf(input.to_vec(), ctx)?, false, ctx)?;
        obj.set("gas", gas, false, ctx)?;
        obj.set("gasUsed", gas_used, false, ctx)?;
        obj.set("gasPrice", gas_price, false, ctx)?;
        obj.set("intrinsicGas", intrinsic_gas, false, ctx)?;
        obj.set("value", to_bigint(value, ctx)?, false, ctx)?;
        obj.set("block", block, false, ctx)?;
        obj.set("output", to_buf(output.to_vec(), ctx)?, false, ctx)?;
        obj.set("time", time, false, ctx)?;
        if let Some(block_hash) = block_hash {
            obj.set("blockHash", to_buf(block_hash.as_bytes().to_vec(), ctx)?, false, ctx)?;
        }
        if let Some(tx_index) = tx_index {
            obj.set("txIndex", tx_index as u64, false, ctx)?;
        }
        if let Some(tx_hash) = tx_hash {
            obj.set("txHash", to_buf(tx_hash.as_bytes().to_vec(), ctx)?, false, ctx)?;
        }

        Ok(obj)
    }
}

/// DB is the object that allows the js inspector to interact with the database.
pub(crate) struct EvmDb {
    db: EvmDBInner,
}

impl EvmDb {
    pub(crate) fn new(state: State, to_db: mpsc::Sender<JsDbRequest>) -> Self {
        Self { db: EvmDBInner { state, to_db } }
    }
}

impl EvmDb {
    pub(crate) fn into_js_object(self, context: &mut Context<'_>) -> JsResult<JsObject> {
        let obj = JsObject::default();

        let db = Gc::new(self.db);
        let exists = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, args, db, ctx| {
                    let val = args.get_or_undefined(0).clone();
                    let db: &EvmDBInner = db.borrow();
                    let acc = db.read_basic(val, ctx)?;
                    let exists = acc.is_some();
                    Ok(JsValue::from(exists))
                },
                db.clone(),
            ),
        )
        .length(1)
        .build();

        let get_balance = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, args, db, ctx| {
                    let val = args.get_or_undefined(0).clone();
                    let db: &EvmDBInner = db.borrow();
                    let acc = db.read_basic(val, ctx)?;
                    let balance = acc.map(|acc| acc.balance).unwrap_or_default();
                    to_bigint(balance, ctx)
                },
                db.clone(),
            ),
        )
        .length(1)
        .build();

        let get_nonce = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, args, db, ctx| {
                    let val = args.get_or_undefined(0).clone();
                    let db: &EvmDBInner = db.borrow();
                    let acc = db.read_basic(val, ctx)?;
                    let nonce = acc.map(|acc| acc.nonce).unwrap_or_default();
                    Ok(JsValue::from(nonce))
                },
                db.clone(),
            ),
        )
        .length(1)
        .build();

        let get_code = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, args, db, ctx| {
                    let val = args.get_or_undefined(0).clone();
                    let db: &EvmDBInner = db.borrow();
                    Ok(db.read_code(val, ctx)?.into())
                },
                db.clone(),
            ),
        )
        .length(1)
        .build();

        let get_state = FunctionObjectBuilder::new(
            context,
            NativeFunction::from_copy_closure_with_captures(
                move |_this, args, db, ctx| {
                    let addr = args.get_or_undefined(0).clone();
                    let slot = args.get_or_undefined(1).clone();
                    let db: &EvmDBInner = db.borrow();
                    Ok(db.read_state(addr, slot, ctx)?.into())
                },
                db,
            ),
        )
        .length(2)
        .build();

        obj.set("getBalance", get_balance, false, context)?;
        obj.set("getNonce", get_nonce, false, context)?;
        obj.set("getCode", get_code, false, context)?;
        obj.set("getState", get_state, false, context)?;
        obj.set("exists", exists, false, context)?;
        Ok(obj)
    }
}

#[derive(Clone)]
struct EvmDBInner {
    state: State,
    to_db: mpsc::Sender<JsDbRequest>,
}

impl EvmDBInner {
    fn read_basic(&self, address: JsValue, ctx: &mut Context<'_>) -> JsResult<Option<Account>> {
        let buf = from_buf(address, ctx)?;
        let address = bytes_to_address(buf);
        if let Some(acc) = self.state.get(&address) {
            return Ok(Some(Account {
                nonce: acc.info.nonce,
                balance: acc.info.balance,
                bytecode_hash: Some(acc.info.code_hash),
            }))
        }
        let (tx, rx) = channel();
        if self.to_db.try_send(JsDbRequest::Basic { address, resp: tx }).is_err() {
            return Err(JsError::from_native(
                JsNativeError::error()
                    .with_message(format!("Failed to read address {address:?} from database",)),
            ))
        }

        match rx.recv() {
            Ok(Ok(maybe_acc)) => Ok(maybe_acc),
            _ => Err(JsError::from_native(
                JsNativeError::error()
                    .with_message(format!("Failed to read address {address:?} from database",)),
            )),
        }
    }

    fn read_code(&self, address: JsValue, ctx: &mut Context<'_>) -> JsResult<JsArrayBuffer> {
        let acc = self.read_basic(address, ctx)?;
        let code_hash = acc.and_then(|acc| acc.bytecode_hash).unwrap_or(KECCAK_EMPTY);
        if code_hash == KECCAK_EMPTY {
            return JsArrayBuffer::new(0, ctx)
        }

        let (tx, rx) = channel();
        if self.to_db.try_send(JsDbRequest::Code { code_hash, resp: tx }).is_err() {
            return Err(JsError::from_native(
                JsNativeError::error()
                    .with_message(format!("Failed to read code hash {code_hash:?} from database",)),
            ))
        }

        let code = match rx.recv() {
            Ok(Ok(code)) => code,
            _ => {
                return Err(JsError::from_native(JsNativeError::error().with_message(format!(
                    "Failed to read code hash {code_hash:?} from database",
                ))))
            }
        };

        to_buf(code.to_vec(), ctx)
    }

    fn read_state(
        &self,
        address: JsValue,
        slot: JsValue,
        ctx: &mut Context<'_>,
    ) -> JsResult<JsArrayBuffer> {
        let buf = from_buf(address, ctx)?;
        let address = bytes_to_address(buf);

        let buf = from_buf(slot, ctx)?;
        let slot = bytes_to_hash(buf);

        let (tx, rx) = channel();
        if self
            .to_db
            .try_send(JsDbRequest::StorageAt { address, index: slot.into(), resp: tx })
            .is_err()
        {
            return Err(JsError::from_native(JsNativeError::error().with_message(format!(
                "Failed to read state for {address:?} at {slot:?} from database",
            ))))
        }

        let value = match rx.recv() {
            Ok(Ok(value)) => value,
            _ => {
                return Err(JsError::from_native(JsNativeError::error().with_message(format!(
                    "Failed to read state for {address:?} at {slot:?} from database",
                ))))
            }
        };
        let value: H256 = value.into();
        to_buf(value.as_bytes().to_vec(), ctx)
    }
}

impl Finalize for EvmDBInner {}

unsafe impl Trace for EvmDBInner {
    empty_trace!();
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
